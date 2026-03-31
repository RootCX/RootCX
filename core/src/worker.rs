use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde_json::Value as JsonValue;
use tokio::process::Command;
use tokio::sync::{Mutex as TokioMutex, broadcast, mpsc, oneshot};
use tracing::{error, info, warn};

use sqlx::PgPool;

use crate::RuntimeError;
use crate::extensions::agents::approvals::{ApprovalRequest, ApprovalResponse, PendingApprovals};
use crate::extensions::agents::supervision::{PolicyDecision, PolicyEvaluator};
use crate::extensions::logs::{LOG_CHANNEL_CAPACITY, LogEntry, emit_log, spawn_output_reader};
use crate::ipc::{AgentBootConfig, AgentInvokePayload, InboundMessage, IpcEvent, IpcReader, IpcWriter, OutboundMessage, PendingRpcs, RpcCaller};
use crate::tools::{AgentDispatcher, ToolRegistry};

const MAX_CRASHES: u32 = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const BACKOFF_BASE: Duration = Duration::from_secs(2);

fn dead() -> RuntimeError {
    RuntimeError::Worker("supervisor actor dead".into())
}

/// Fleet-wide event envelope for SSE fan-out.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FleetEvent {
    pub app_id: String,
    pub session_id: String,
    #[serde(flatten)]
    pub event: AgentEvent,
}

/// Events streamed from an agent worker back to the invoke route.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "_event", rename_all = "snake_case")]
pub enum AgentEvent {
    Chunk { delta: String },
    Done { response: String, tokens: Option<u64> },
    Error { error: String },
    ToolCallStarted { call_id: String, tool_name: String, input: JsonValue },
    ToolCallCompleted { call_id: String, tool_name: String, output: Option<JsonValue>, error: Option<String>, duration_ms: u64 },
    ApprovalRequired { approval_id: String, tool_name: String, args: JsonValue, reason: String },
    SessionCompacted { summary: String },
    SubAgentChunk { app_id: String, delta: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerStatus {
    Starting,
    Running,
    Stopping,
    Stopped,
    Crashed,
}

pub enum SupervisorCommand {
    Start,
    Stop {
        reply: oneshot::Sender<()>,
    },
    Rpc {
        id: String,
        method: String,
        params: JsonValue,
        caller: Option<RpcCaller>,
        reply: oneshot::Sender<Result<JsonValue, String>>,
    },
    Job {
        id: String,
        payload: JsonValue,
        caller: Option<RpcCaller>,
    },
    AgentInvoke {
        payload: AgentInvokePayload,
        stream_tx: mpsc::Sender<AgentEvent>,
    },
    GetStatus {
        reply: oneshot::Sender<WorkerStatus>,
    },
}

pub struct WorkerConfig {
    pub app_id: String,
    pub entry_point: PathBuf,
    pub working_dir: PathBuf,
    pub credentials: HashMap<String, String>,
    pub runtime_url: String,
    pub database_url: String,
    pub pool: PgPool,
    pub js_runtime: PathBuf,
    pub prelude_path: PathBuf,
    pub tool_registry: Arc<ToolRegistry>,
    pub pending_approvals: PendingApprovals,
    pub agent_dispatch: Option<Arc<dyn AgentDispatcher>>,
    pub integration_caller: Option<Arc<dyn crate::tools::IntegrationCaller>>,
    pub agent_boot_config: Option<AgentBootConfig>,
    pub supervision: Option<rootcx_types::SupervisionConfig>,
}

#[derive(Clone)]
pub struct SupervisorHandle {
    tx: mpsc::Sender<SupervisorCommand>,
    log_tx: broadcast::Sender<LogEntry>,
}

impl SupervisorHandle {
    async fn send(&self, cmd: SupervisorCommand) -> Result<(), RuntimeError> {
        self.tx.send(cmd).await.map_err(|_| dead())
    }

    pub async fn start(&self) -> Result<(), RuntimeError> {
        self.send(SupervisorCommand::Start).await
    }

    pub async fn stop(&self) -> Result<(), RuntimeError> {
        let (reply, rx) = oneshot::channel();
        self.send(SupervisorCommand::Stop { reply }).await?;
        rx.await.map_err(|_| dead())
    }

    pub async fn rpc(
        &self,
        id: String,
        method: String,
        params: JsonValue,
        caller: Option<RpcCaller>,
    ) -> Result<JsonValue, RuntimeError> {
        let (reply, rx) = oneshot::channel();
        self.send(SupervisorCommand::Rpc { id, method, params, caller, reply }).await?;
        rx.await.map_err(|_| dead())?.map_err(RuntimeError::Worker)
    }

    pub async fn dispatch_job(&self, id: String, payload: JsonValue, caller: Option<RpcCaller>) -> Result<(), RuntimeError> {
        self.send(SupervisorCommand::Job { id, payload, caller }).await
    }

    pub async fn status(&self) -> Result<WorkerStatus, RuntimeError> {
        let (reply, rx) = oneshot::channel();
        self.send(SupervisorCommand::GetStatus { reply }).await?;
        rx.await.map_err(|_| dead())
    }

    pub async fn agent_invoke(
        &self,
        payload: AgentInvokePayload,
    ) -> Result<mpsc::Receiver<AgentEvent>, RuntimeError> {
        let (stream_tx, stream_rx) = mpsc::channel(64);
        self.send(SupervisorCommand::AgentInvoke { payload, stream_tx }).await?;
        Ok(stream_rx)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<LogEntry> {
        self.log_tx.subscribe()
    }
}

pub fn spawn_supervisor(config: WorkerConfig) -> SupervisorHandle {
    let (tx, rx) = mpsc::channel(64);
    let (log_tx, _) = broadcast::channel(LOG_CHANNEL_CAPACITY);
    tokio::spawn(supervisor_loop(config, rx, log_tx.clone()));
    SupervisorHandle { tx, log_tx }
}

async fn supervisor_loop(
    config: WorkerConfig,
    mut cmd_rx: mpsc::Receiver<SupervisorCommand>,
    log_tx: broadcast::Sender<LogEntry>,
) {
    let app_id = config.app_id.clone();
    let mut status = WorkerStatus::Stopped;
    let mut child: Option<AsyncGroupChild> = None;
    let mut ipc_writer: Option<IpcWriter> = None;
    let mut ipc_reader: Option<IpcReader> = None;
    let mut pending_rpcs = PendingRpcs::default();
    let mut pending_agent_streams: HashMap<String, mpsc::Sender<AgentEvent>> = HashMap::new();
    let mut policy_evaluators: HashMap<String, Arc<TokioMutex<PolicyEvaluator>>> = HashMap::new();
    let mut sub_invocations: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut invoker_user_ids: HashMap<String, uuid::Uuid> = HashMap::new();
    let pending_approvals = config.pending_approvals.clone();
    let mut crash_times: Vec<Instant> = Vec::new();
    let mut restart_count: u32 = 0;
    let mut output_handles = Vec::new();

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<OutboundMessage>(64);

    info!(app_id = %app_id, "supervisor started");

    loop {
        tokio::select! {
            Some(msg) = outbound_rx.recv() => {
                if let Some(ref mut w) = ipc_writer {
                    let _ = w.send(&msg).await;
                }
            }

            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    SupervisorCommand::Start => {
                        if matches!(status, WorkerStatus::Running | WorkerStatus::Starting) {
                            continue;
                        }
                        if status == WorkerStatus::Crashed {
                            warn!(app_id = %app_id, "cannot start crashed worker");
                            continue;
                        }
                        match spawn_worker(&config).await {
                            Ok((c, writer, reader, stderr)) => {
                                child = Some(c);
                                ipc_writer = Some(writer);
                                ipc_reader = Some(reader);
                                output_handles.push(spawn_output_reader(stderr, "stderr", log_tx.clone()));
                                status = WorkerStatus::Running;
                                restart_count = 0;
                                info!(app_id = %app_id, "worker started");
                                emit_log(&log_tx, "system", "worker started");
                            }
                            Err(e) => {
                                error!(app_id = %app_id, "spawn failed: {e}");
                                emit_log(&log_tx, "system", format!("spawn failed: {e}"));
                                status = WorkerStatus::Crashed;
                            }
                        }
                    }

                    SupervisorCommand::Stop { reply } => {
                        if let Some(ref mut w) = ipc_writer {
                            let _ = w.send(&OutboundMessage::Shutdown).await;
                        }
                        ipc_writer = None;
                        ipc_reader = None;
                        for h in output_handles.drain(..) { h.abort(); }
                        kill_child(&mut child).await;
                        for (_sid, tx) in pending_agent_streams.drain() {
                            let _ = tx.send(AgentEvent::Error { error: "worker stopped".into() }).await;
                        }
                        policy_evaluators.clear();
                        status = WorkerStatus::Stopped;
                        crash_times.clear();
                        info!(app_id = %app_id, "worker stopped");
                        emit_log(&log_tx, "system", "worker stopped");
                        let _ = reply.send(());
                    }

                    SupervisorCommand::Rpc { id, method, params, caller, reply } => {
                        if status != WorkerStatus::Running {
                            let _ = reply.send(Err("worker not running".into()));
                            continue;
                        }
                        let rx = pending_rpcs.register(id.clone());
                        if let Some(ref mut w) = ipc_writer
                            && let Err(e) = w.send(&OutboundMessage::Rpc { id: id.clone(), method, params, caller }).await {
                                pending_rpcs.resolve(&id, Err(e.to_string()));
                            }
                        tokio::spawn(async move {
                            let result = match tokio::time::timeout(Duration::from_secs(30), rx).await {
                                Ok(Ok(r)) => r,
                                Ok(Err(_)) => Err("rpc channel dropped".into()),
                                Err(_) => Err("rpc timeout (30s)".into()),
                            };
                            let _ = reply.send(result);
                        });
                    }

                    SupervisorCommand::Job { id, payload, caller } => {
                        if status != WorkerStatus::Running {
                            warn!(app_id = %app_id, job_id = %id, "worker not running");
                            continue;
                        }
                        if let Some(ref mut w) = ipc_writer
                            && let Err(e) = w.send(&OutboundMessage::Job { id: id.clone(), payload, caller }).await {
                                error!(app_id = %app_id, job_id = %id, "send failed: {e}");
                            }
                    }

                    SupervisorCommand::AgentInvoke { payload, stream_tx } => {
                        if status != WorkerStatus::Running {
                            let _ = stream_tx.send(AgentEvent::Error {
                                error: "worker not running".into(),
                            }).await;
                            continue;
                        }
                        let invoke_id = payload.invoke_id.clone();

                        if let Some(ref supervision) = config.supervision {
                            policy_evaluators.insert(invoke_id.clone(),
                                Arc::new(TokioMutex::new(PolicyEvaluator::new(supervision.clone()))));
                        }
                        if payload.is_sub_invoke { sub_invocations.insert(invoke_id.clone()); }
                        if let Some(uid) = payload.invoker_user_id { invoker_user_ids.insert(invoke_id.clone(), uid); }

                        pending_agent_streams.insert(invoke_id.clone(), stream_tx);
                        if let Some(ref mut w) = ipc_writer {
                            if let Err(e) = w.send(&OutboundMessage::AgentInvoke(payload)).await {
                                if let Some(tx) = pending_agent_streams.remove(&invoke_id) {
                                    let _ = tx.send(AgentEvent::Error { error: e.to_string() }).await;
                                }
                                policy_evaluators.remove(&invoke_id);
                            }
                        }
                    }

                    SupervisorCommand::GetStatus { reply } => {
                        let _ = reply.send(status.clone());
                    }
                }
            }

            event = async {
                match ipc_reader.as_mut() {
                    Some(reader) => reader.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match event {
                    Some(IpcEvent::Message(msg)) => match msg {
                        InboundMessage::RpcResponse { id, result, error } => {
                            pending_rpcs.resolve(&id, match error {
                                Some(e) => Err(e),
                                None => Ok(result.unwrap_or(JsonValue::Null)),
                            });
                        }
                        InboundMessage::JobResult { id, error } => {
                            if let Ok(msg_id) = id.parse::<i64>() {
                                if let Some(e) = error {
                                    warn!(app_id = %app_id, msg_id, "job failed: {e}");
                                    let _ = crate::jobs::fail(&config.pool, msg_id).await;
                                } else {
                                    info!(app_id = %app_id, msg_id, "job completed");
                                    let _ = crate::jobs::complete(&config.pool, msg_id).await;
                                }
                            } else {
                                warn!(app_id = %app_id, "invalid msg_id: {id}");
                            }
                        }
                        InboundMessage::Log { level, message } => {
                            let message = if message.len() > 8192 { &message[..8192] } else { &message };
                            match level.as_str() {
                                "error" => error!(app_id = %app_id, "[worker] {message}"),
                                "warn" => warn!(app_id = %app_id, "[worker] {message}"),
                                "debug" => tracing::debug!(app_id = %app_id, "[worker] {message}"),
                                _ => info!(app_id = %app_id, "[worker] {message}"),
                            }
                            emit_log(&log_tx, &level, message);
                        }
                        InboundMessage::AgentChunk { invoke_id, delta } => {
                            if let Some(tx) = pending_agent_streams.get(&invoke_id) {
                                if tx.send(AgentEvent::Chunk { delta }).await.is_err() {
                                    pending_agent_streams.remove(&invoke_id);
                                }
                            }
                        }
                        InboundMessage::AgentDone { invoke_id, response, tokens } => {
                            policy_evaluators.remove(&invoke_id);
                            sub_invocations.remove(&invoke_id);
                            invoker_user_ids.remove(&invoke_id);
                            if let Some(tx) = pending_agent_streams.remove(&invoke_id) {
                                let _ = tx.send(AgentEvent::Done { response, tokens }).await;
                            }
                        }
                        InboundMessage::AgentError { invoke_id, error } => {
                            policy_evaluators.remove(&invoke_id);
                            sub_invocations.remove(&invoke_id);
                            invoker_user_ids.remove(&invoke_id);
                            if let Some(tx) = pending_agent_streams.remove(&invoke_id) {
                                let _ = tx.send(AgentEvent::Error { error }).await;
                            }
                        }
                        InboundMessage::AgentToolCall { invoke_id, call_id, tool_name, args } => {
                            if let Some(tx) = pending_agent_streams.get(&invoke_id) {
                                let _ = tx.send(AgentEvent::ToolCallStarted {
                                    call_id: call_id.clone(),
                                    tool_name: tool_name.clone(),
                                    input: args.clone(),
                                }).await;
                            }

                            let tool = config.tool_registry.get(&tool_name);
                            let pool = config.pool.clone();
                            let aid = config.app_id.clone();
                            // Nesting guard: sub-agents cannot spawn sub-agents
                            let dispatch = if sub_invocations.contains(&invoke_id) { None } else { config.agent_dispatch.clone() };
                            let int_caller = config.integration_caller.clone();
                            let invoker_uid = invoker_user_ids.get(&invoke_id).copied();
                            let out_tx = outbound_tx.clone();
                            let evaluator = policy_evaluators.get(&invoke_id).cloned();
                            let approvals_ref = pending_approvals.clone();
                            let stream_tx = pending_agent_streams.get(&invoke_id).cloned();

                            tokio::spawn(async move {
                                if let Some(ref eval) = evaluator {
                                    match eval.lock().await.evaluate(&tool_name, &args) {
                                        PolicyDecision::Allow => {}
                                        PolicyDecision::RateLimited { retry_after_secs } => {
                                            let err = format!("rate limited: retry after {retry_after_secs}s");
                                            let _ = out_tx.send(OutboundMessage::AgentToolResult {
                                                invoke_id, call_id: call_id.clone(), result: None, error: Some(err.clone()),
                                            }).await;
                                            if let Some(tx) = stream_tx {
                                                let _ = tx.send(AgentEvent::ToolCallCompleted {
                                                    call_id, tool_name, output: None, error: Some(err), duration_ms: 0,
                                                }).await;
                                            }
                                            return;
                                        }
                                        PolicyDecision::RequiresApproval { reason } => {
                                            let approval_id = uuid::Uuid::new_v4().to_string();
                                            if let Some(ref tx) = stream_tx {
                                                let _ = tx.send(AgentEvent::ApprovalRequired {
                                                    approval_id: approval_id.clone(), tool_name: tool_name.clone(),
                                                    args: args.clone(), reason: reason.clone(),
                                                }).await;
                                            }
                                            let rx = approvals_ref.request(ApprovalRequest {
                                                approval_id, app_id: aid.clone(), session_id: String::new(),
                                                invoke_id: invoke_id.clone(), call_id: call_id.clone(),
                                                tool_name: tool_name.clone(), args: args.clone(), reason,
                                                created_at: chrono::Utc::now().to_rfc3339(),
                                            }).await;
                                            match rx.await {
                                                Ok(ApprovalResponse::Approved) => {}
                                                Ok(ApprovalResponse::Rejected { reason }) => {
                                                    let err = format!("rejected: {reason}");
                                                    let _ = out_tx.send(OutboundMessage::AgentToolResult {
                                                        invoke_id, call_id: call_id.clone(), result: None, error: Some(err.clone()),
                                                    }).await;
                                                    if let Some(tx) = stream_tx {
                                                        let _ = tx.send(AgentEvent::ToolCallCompleted {
                                                            call_id, tool_name, output: None, error: Some(err), duration_ms: 0,
                                                        }).await;
                                                    }
                                                    return;
                                                }
                                                Err(_) => {
                                                    let err = "approval channel dropped".to_string();
                                                    let _ = out_tx.send(OutboundMessage::AgentToolResult {
                                                        invoke_id, call_id: call_id.clone(), result: None, error: Some(err.clone()),
                                                    }).await;
                                                    if let Some(tx) = stream_tx {
                                                        let _ = tx.send(AgentEvent::ToolCallCompleted {
                                                            call_id, tool_name, output: None, error: Some(err), duration_ms: 0,
                                                        }).await;
                                                    }
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                }

                                let agent_uid = crate::extensions::agents::agent_user_id(&aid);
                                let permissions = crate::extensions::rbac::policy::resolve_permissions(&pool, &aid, agent_uid)
                                    .await.map(|(_, p)| p).unwrap_or_default();

                                crate::tool_executor::execute(
                                    tool, tool_name, args, aid, agent_uid, invoker_uid,
                                    permissions, pool, dispatch, int_caller,
                                    out_tx, stream_tx, invoke_id, call_id,
                                ).await;
                            });
                        }
                        InboundMessage::AgentSessionCompacted { invoke_id, summary } => {
                            if let Some(tx) = pending_agent_streams.get(&invoke_id) {
                                let _ = tx.send(AgentEvent::SessionCompacted { summary }).await;
                            }
                            info!(app_id = %app_id, "agent session compacted");
                        }
                        InboundMessage::CollectionOp { id, op, entity, data } => {
                            let pool = config.pool.clone();
                            let aid = config.app_id.clone();
                            let tx = outbound_tx.clone();
                            tokio::spawn(async move {
                                let (result, error) = match collection_op(&pool, &aid, &op, &entity, data).await {
                                    Ok(v) => (Some(v), None),
                                    Err(e) => (None, Some(e)),
                                };
                                let _ = tx.send(OutboundMessage::CollectionOpResult { id, result, error }).await;
                            });
                        }
                        InboundMessage::Event { name, data } => {
                            info!(app_id = %app_id, event = %name, "worker event");
                            emit_log(&log_tx, "event", format!("{name}: {data}"));
                        }
                        _ => {}
                    },
                    Some(IpcEvent::Output(line)) => {
                        emit_log(&log_tx, "stdout", &line);
                    }
                    None if matches!(status, WorkerStatus::Stopping | WorkerStatus::Stopped) => {
                        continue;
                    }
                    None => {
                        if let Some(ref mut c) = child {
                            let exit = AsyncGroupChild::wait(c).await;
                            warn!(app_id = %app_id, ?exit, "worker exited unexpectedly");
                            emit_log(&log_tx, "system", "worker crashed");
                        }
                        child = None;
                        ipc_writer = None;
                        ipc_reader = None;
                        for h in output_handles.drain(..) { h.abort(); }

                        for (_sid, tx) in pending_agent_streams.drain() {
                            let _ = tx.send(AgentEvent::Error {
                                error: "worker crashed".into(),
                            }).await;
                        }

                        let now = Instant::now();
                        crash_times.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
                        crash_times.push(now);
                        restart_count += 1;

                        if crash_times.len() as u32 >= MAX_CRASHES {
                            error!(app_id = %app_id, "crash loop ({MAX_CRASHES} in {CRASH_WINDOW:?}), giving up");
                            emit_log(&log_tx, "system", format!("crash loop ({MAX_CRASHES} crashes in {CRASH_WINDOW:?}), giving up"));
                            status = WorkerStatus::Crashed;
                            continue;
                        }

                        let delay = backoff_delay(restart_count);

                        // Interruptible backoff: stop commands can interrupt the wait
                        if !delay.is_zero() {
                            info!(app_id = %app_id, delay_ms = delay.as_millis() as u64, "backoff");
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                Some(cmd) = cmd_rx.recv() => {
                                    match cmd {
                                        SupervisorCommand::Stop { reply } => {
                                            status = WorkerStatus::Stopped;
                                            crash_times.clear();
                                            info!(app_id = %app_id, "worker stopped during backoff");
                                            emit_log(&log_tx, "system", "worker stopped");
                                            let _ = reply.send(());
                                            continue;
                                        }
                                        SupervisorCommand::GetStatus { reply } => {
                                            let _ = reply.send(status.clone());
                                            // Fall through to restart
                                        }
                                        _ => {} // Ignore start/rpc/job during backoff
                                    }
                                }
                            }
                        }

                        info!(app_id = %app_id, attempt = restart_count, "restarting worker");
                        emit_log(&log_tx, "system", format!("restarting worker (attempt {restart_count})"));
                        match spawn_worker(&config).await {
                            Ok((c, writer, reader, stderr)) => {
                                child = Some(c);
                                ipc_writer = Some(writer);
                                ipc_reader = Some(reader);
                                output_handles.push(spawn_output_reader(stderr, "stderr", log_tx.clone()));
                                status = WorkerStatus::Running;
                                emit_log(&log_tx, "system", "worker restarted");
                            }
                            Err(e) => {
                                error!(app_id = %app_id, "restart failed: {e}");
                                emit_log(&log_tx, "system", format!("restart failed: {e}"));
                                status = WorkerStatus::Crashed;
                            }
                        }
                    }
                }
            }
        }

        if cmd_rx.is_closed() && child.is_none() {
            break;
        }
    }

    kill_child(&mut child).await;
    info!(app_id = %app_id, "supervisor exited");
}

async fn kill_child(child: &mut Option<AsyncGroupChild>) {
    if let Some(c) = child.as_mut() {
        let _ = c.start_kill();
        let _ = c.wait().await;
    }
    *child = None;
}

async fn spawn_worker(
    config: &WorkerConfig,
) -> Result<(AsyncGroupChild, IpcWriter, IpcReader, tokio::process::ChildStderr), RuntimeError> {
    let bin = &config.js_runtime;
    info!(app_id = %config.app_id, bin = %bin.display(), entry = %config.entry_point.display(), "spawning worker");

    let mut cmd = Command::new(bin);
    cmd.arg("--preload").arg(&config.prelude_path)
        .arg(&config.entry_point)
        .current_dir(&config.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("ROOTCX_APP_ID", &config.app_id)
        .env("ROOTCX_RUNTIME_URL", &config.runtime_url);

    let mut child = cmd.group_spawn().map_err(|e| RuntimeError::Worker(format!("spawn failed: {e}")))?;

    let stdin = child.inner().stdin.take().ok_or_else(|| RuntimeError::Worker("no stdin".into()))?;
    let stdout = child.inner().stdout.take().ok_or_else(|| RuntimeError::Worker("no stdout".into()))?;
    let stderr = child.inner().stderr.take().ok_or_else(|| RuntimeError::Worker("no stderr".into()))?;

    let mut writer = IpcWriter::new(stdin);
    writer.send(&OutboundMessage::Discover {
        app_id: config.app_id.clone(),
        runtime_url: config.runtime_url.clone(),
        database_url: config.database_url.clone(),
        credentials: config.credentials.clone(),
        agent_config: config.agent_boot_config.clone(),
    }).await?;

    Ok((child, writer, IpcReader::new(stdout), stderr))
}

async fn collection_op(pool: &PgPool, app_id: &str, op: &str, entity: &str, data: JsonValue) -> Result<JsonValue, String> {
    use crate::manifest::{field_type_map, quote_ident};
    use crate::routes::crud::{bind_typed, table};

    let obj = data.as_object().ok_or("data must be a JSON object")?;
    let tbl = table(app_id, entity);
    let types = field_type_map(pool, app_id, entity).await.map_err(|e| e.to_string())?;

    let sql = match op {
        "insert" => {
            let cols: Vec<_> = obj.keys().map(|k| quote_ident(k)).collect();
            let phs: Vec<_> = (1..=cols.len()).map(|i| format!("${i}")).collect();
            format!("INSERT INTO {tbl} ({}) VALUES ({}) RETURNING to_jsonb({tbl}.*) AS row", cols.join(","), phs.join(","))
        }
        "update" => {
            let mut idx = 1usize;
            let sets: Vec<_> = obj.keys().filter(|k| *k != "id").map(|k| { let s = format!("{} = ${idx}", quote_ident(k)); idx += 1; s }).collect();
            if sets.is_empty() { return Err("no fields to update".into()); }
            format!("UPDATE {tbl} SET {} WHERE id = ${idx} RETURNING to_jsonb({tbl}.*) AS row", sets.join(","))
        }
        _ => return Err(format!("unsupported op: {op}")),
    };

    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for (k, v) in obj.iter() {
        if op == "update" && k == "id" { continue; }
        query = bind_typed(query, v, types.get(k.as_str()));
    }
    if op == "update" {
        let id = obj.get("id").and_then(|v| v.as_str()).ok_or("id required for update")?;
        query = query.bind(id);
    }
    let (row,) = query.fetch_one(pool).await.map_err(|e| e.to_string())?;
    Ok(row)
}

fn backoff_delay(restart_count: u32) -> Duration {
    if restart_count <= 1 { Duration::ZERO } else { BACKOFF_BASE * 2u32.saturating_pow(restart_count - 2) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_delays() {
        for (count, secs) in [(0, 0), (1, 0), (2, 2), (3, 4), (4, 8)] {
            assert_eq!(backoff_delay(count), Duration::from_secs(secs), "backoff_delay({count})");
        }
        let _ = backoff_delay(34); // saturates without panic
    }
}
