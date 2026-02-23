use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use command_group::{AsyncCommandGroup, AsyncGroupChild};
use serde_json::Value as JsonValue;
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{error, info, warn};

use sqlx::PgPool;

use crate::RuntimeError;
use crate::extensions::logs::{LOG_CHANNEL_CAPACITY, LogEntry, emit_log, spawn_output_reader};
use crate::ipc::{InboundMessage, IpcEvent, IpcReader, IpcWriter, OutboundMessage, PendingRpcs, RpcCaller};

const MAX_CRASHES: u32 = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(60);
const BACKOFF_BASE: Duration = Duration::from_secs(2);

fn dead() -> RuntimeError {
    RuntimeError::Worker("supervisor actor dead".into())
}

/// Events streamed from an agent worker back to the invoke route.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    Chunk { delta: String },
    Done { response: String, tokens: Option<u64> },
    Error { error: String },
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
    },
    AgentInvoke {
        session_id: String,
        message: String,
        system_prompt: String,
        config: JsonValue,
        auth_token: String,
        history: Vec<JsonValue>,
        caller: Option<RpcCaller>,
        stream_tx: mpsc::Sender<AgentEvent>,
    },
    GetStatus {
        reply: oneshot::Sender<WorkerStatus>,
    },
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub app_id: String,
    pub entry_point: PathBuf,
    pub working_dir: PathBuf,
    pub env: HashMap<String, String>,
    pub runtime_url: String,
    pub pool: PgPool,
    pub js_runtime: PathBuf,
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

    pub async fn dispatch_job(&self, id: String, payload: JsonValue) -> Result<(), RuntimeError> {
        self.send(SupervisorCommand::Job { id, payload }).await
    }

    pub async fn status(&self) -> Result<WorkerStatus, RuntimeError> {
        let (reply, rx) = oneshot::channel();
        self.send(SupervisorCommand::GetStatus { reply }).await?;
        rx.await.map_err(|_| dead())
    }

    pub async fn agent_invoke(
        &self,
        session_id: String,
        message: String,
        system_prompt: String,
        config: JsonValue,
        auth_token: String,
        history: Vec<JsonValue>,
        caller: Option<RpcCaller>,
    ) -> Result<mpsc::Receiver<AgentEvent>, RuntimeError> {
        let (stream_tx, stream_rx) = mpsc::channel(64);
        self.send(SupervisorCommand::AgentInvoke {
            session_id, message, system_prompt,
            config, auth_token, history, caller, stream_tx,
        }).await?;
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
    let mut pending_rpcs = PendingRpcs::new();
    let mut pending_agent_streams: HashMap<String, mpsc::Sender<AgentEvent>> = HashMap::new();
    let mut crash_times: Vec<Instant> = Vec::new();
    let mut restart_count: u32 = 0;
    let mut output_handles: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    info!(app_id = %app_id, "supervisor started");

    loop {
        tokio::select! {
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
                        // Spawn timeout watcher
                        tokio::spawn(async move {
                            let result = match tokio::time::timeout(Duration::from_secs(30), rx).await {
                                Ok(Ok(r)) => r,
                                Ok(Err(_)) => Err("rpc channel dropped".into()),
                                Err(_) => Err("rpc timeout (30s)".into()),
                            };
                            let _ = reply.send(result);
                        });
                    }

                    SupervisorCommand::Job { id, payload } => {
                        if status != WorkerStatus::Running {
                            warn!(app_id = %app_id, job_id = %id, "worker not running");
                            continue;
                        }
                        if let Some(ref mut w) = ipc_writer
                            && let Err(e) = w.send(&OutboundMessage::Job { id: id.clone(), payload }).await {
                                error!(app_id = %app_id, job_id = %id, "send failed: {e}");
                            }
                    }

                    SupervisorCommand::AgentInvoke {
                        session_id, message, system_prompt,
                        config, auth_token, history, caller, stream_tx,
                    } => {
                        if status != WorkerStatus::Running {
                            let _ = stream_tx.send(AgentEvent::Error {
                                error: "worker not running".into(),
                            }).await;
                            continue;
                        }
                        pending_agent_streams.insert(session_id.clone(), stream_tx);
                        if let Some(ref mut w) = ipc_writer {
                            if let Err(e) = w.send(&OutboundMessage::AgentInvoke {
                                session_id: session_id.clone(),
                                message, system_prompt, config, auth_token, history, caller,
                            }).await {
                                if let Some(tx) = pending_agent_streams.remove(&session_id) {
                                    let _ = tx.send(AgentEvent::Error { error: e.to_string() }).await;
                                }
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
                        InboundMessage::Discover { capabilities } => {
                            info!(app_id = %app_id, ?capabilities, "worker discovered");
                            if let Some(ref mut w) = ipc_writer {
                                let _ = w.send(&OutboundMessage::Discover {
                                    app_id: config.app_id.clone(),
                                    runtime_url: config.runtime_url.clone(),
                                }).await;
                            }
                        }
                        InboundMessage::RpcResponse { id, result, error } => {
                            pending_rpcs.resolve(&id, match error {
                                Some(e) => Err(e),
                                None => Ok(result.unwrap_or(JsonValue::Null)),
                            });
                        }
                        InboundMessage::JobResult { id, result, error } => {
                            if let Ok(job_id) = uuid::Uuid::parse_str(&id) {
                                if let Some(e) = error {
                                    warn!(app_id = %app_id, job_id = %id, "job failed: {e}");
                                    let _ = crate::jobs::fail(&config.pool, job_id, &e).await;
                                } else {
                                    info!(app_id = %app_id, job_id = %id, "job completed");
                                    let _ = crate::jobs::complete(&config.pool, job_id, result.unwrap_or(JsonValue::Null)).await;
                                }
                            } else {
                                warn!(app_id = %app_id, "invalid job id: {id}");
                            }
                        }
                        InboundMessage::Log { level, message } => {
                            match level.as_str() {
                                "error" => error!(app_id = %app_id, "[worker] {message}"),
                                "warn" => warn!(app_id = %app_id, "[worker] {message}"),
                                "debug" => tracing::debug!(app_id = %app_id, "[worker] {message}"),
                                _ => info!(app_id = %app_id, "[worker] {message}"),
                            }
                            emit_log(&log_tx, &level, &message);
                        }
                        InboundMessage::AgentChunk { session_id, delta } => {
                            if let Some(tx) = pending_agent_streams.get(&session_id) {
                                if tx.send(AgentEvent::Chunk { delta }).await.is_err() {
                                    pending_agent_streams.remove(&session_id);
                                }
                            }
                        }
                        InboundMessage::AgentDone { session_id, response, tokens } => {
                            if let Some(tx) = pending_agent_streams.remove(&session_id) {
                                let _ = tx.send(AgentEvent::Done { response, tokens }).await;
                            }
                        }
                        InboundMessage::AgentError { session_id, error } => {
                            if let Some(tx) = pending_agent_streams.remove(&session_id) {
                                let _ = tx.send(AgentEvent::Error { error }).await;
                            }
                        }
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
                                    // Process stop/status during backoff
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
    cmd.arg(&config.entry_point)
        .current_dir(&config.working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("ROOTCX_APP_ID", &config.app_id)
        .env("ROOTCX_RUNTIME_URL", &config.runtime_url);

    for (k, v) in &config.env {
        cmd.env(k, v);
    }

    let mut child = cmd.group_spawn().map_err(|e| RuntimeError::Worker(format!("spawn failed: {e}")))?;

    let stdin = child.inner().stdin.take().ok_or_else(|| RuntimeError::Worker("no stdin".into()))?;
    let stdout = child.inner().stdout.take().ok_or_else(|| RuntimeError::Worker("no stdout".into()))?;
    let stderr = child.inner().stderr.take().ok_or_else(|| RuntimeError::Worker("no stderr".into()))?;

    Ok((child, IpcWriter::new(stdin), IpcReader::new(stdout), stderr))
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
