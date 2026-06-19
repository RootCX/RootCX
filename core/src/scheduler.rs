use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::extensions::agents::persistence;
use crate::extensions::workflows::events::WorkflowEvents;
use crate::ipc::{AgentInvokePayload, LlmModelRef};
use crate::tools::ToolRegistry;
use crate::{jobs, worker_manager::WorkerManager};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct SchedulerHandle {
    pub wake: Arc<Notify>,
    pub cancel: CancellationToken,
}

async fn dispatch_agent_job(
    pool: &PgPool,
    wm: &Arc<WorkerManager>,
    msg_id: i64,
    target_app: &str,
    message: String,
    invoker_user_id: Option<uuid::Uuid>,
    label: &'static str,
) {
    // Single owned-automation gate (B1): owner present + enabled + valid
    // delegation + still holds app:{id}:invoke. Fail-closed.
    if let Err(denied) = crate::governance::triggers::fire_gate::assert_can_fire(
        pool, invoker_user_id, target_app,
    ).await {
        warn!(msg_id, app_id = %target_app, "{label} agent denied: {denied}");
        let _ = jobs::fail(pool, msg_id).await;
        return;
    }

    let llm = crate::routes::llm_models::fetch_default_llm(pool).await
        .ok().flatten()
        .map(|(provider, model)| LlmModelRef { provider, model });

    let session_id = uuid::Uuid::new_v4().to_string();
    let user_message = message.clone();

    let invoke_payload = AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        message,
        history: vec![],
        is_sub_invoke: false,
        llm,
        invoker_user_id,
        attachments: None,
        task_scope: Some(vec![format!("app:{target_app}:*")]),
    };

    let system_user = uuid::Uuid::nil();
    let _ = persistence::ensure_session(pool, &session_id, target_app, system_user).await;
    let _ = persistence::persist_message(pool, &session_id, "user", &user_message, None, false).await;

    match wm.agent_invoke(target_app, invoke_payload, None).await {
        Ok(mut rx) => {
            let pool_c = pool.clone();
            let target_app_c = target_app.to_string();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    match event {
                        crate::worker::AgentEvent::ToolCallStarted { call_id, tool_name, input } => {
                            let _ = persistence::persist_tool_call_start(&pool_c, &session_id, &call_id, &tool_name, &input).await;
                        }
                        crate::worker::AgentEvent::ToolCallCompleted { call_id, output, error, duration_ms, .. } => {
                            let _ = persistence::persist_tool_call_end(&pool_c, &call_id, output.as_ref(), error.as_deref(), duration_ms).await;
                        }
                        crate::worker::AgentEvent::Done { response, tokens } => {
                            let _ = persistence::finalize_session(&pool_c, &session_id, &user_message, &response, tokens).await;
                            let _ = jobs::complete(&pool_c, msg_id).await;
                            info!(msg_id, app_id = %target_app_c, %session_id, "{label} agent completed");
                            return;
                        }
                        crate::worker::AgentEvent::Error { error } => {
                            error!(msg_id, app_id = %target_app_c, "{label} agent error: {error}");
                            let _ = jobs::fail(&pool_c, msg_id).await;
                            return;
                        }
                        _ => {}
                    }
                }
                let _ = jobs::fail(&pool_c, msg_id).await;
            });
        }
        Err(e) => {
            warn!(msg_id, "{label} agent dispatch failed: {e}");
            let _ = jobs::fail(pool, msg_id).await;
        }
    }
}

/// A workflow message is dead-lettered after this many deliveries (lease expiries).
const MAX_DELIVERIES: i32 = 5;

/// Message ids with a live run task. The runtime runs a single scheduler, so an
/// in-memory set is enough to enforce single-flight per lease and stop a
/// redelivered message from starting a second concurrent run of the same execution.
type InFlight = Arc<Mutex<HashSet<i64>>>;

/// Clears the in-flight mark on drop — including panic, so a crashed run frees its
/// lease for a clean redelivery/resume rather than wedging it forever.
struct InFlightGuard { set: InFlight, msg_id: i64 }
impl Drop for InFlightGuard {
    fn drop(&mut self) { self.set.lock().unwrap().remove(&self.msg_id); }
}

/// Resolve the responsible human's permissions, or drop the message (terminal —
/// perms won't change on redelivery).
async fn perms_or_fail(pool: &PgPool, msg_id: i64, uid: uuid::Uuid) -> Option<Vec<String>> {
    match crate::governance::authority::resolve_permissions(pool, uid).await {
        Ok((_, perms)) => Some(perms),
        Err(e) => {
            warn!(msg_id, "workflow perms: {e:?}");
            let _ = jobs::fail(pool, msg_id).await;
            None
        }
    }
}

/// Drive a non-terminal execution to `failed` and release its stream channel. The
/// status guard makes it a no-op once the run already finished (avoids clobbering a
/// concurrent terminal write).
async fn fail_execution(pool: &PgPool, events: &WorkflowEvents, exec_id: uuid::Uuid, reason: &str) {
    let _ = sqlx::query(
        "UPDATE rootcx_system.workflow_executions
         SET status = 'failed', error = $2, finished_at = now(), lease_msg_id = NULL
         WHERE id = $1 AND status NOT IN ('succeeded', 'failed', 'canceled')",
    ).bind(exec_id).bind(reason).execute(pool).await;
    events.close(exec_id);
}

/// Poison guard: fail the bound execution (if any) and move the message to the DLQ.
async fn dead_letter_workflow(pool: &PgPool, events: &WorkflowEvents, msg_id: i64, exec_id: Option<uuid::Uuid>, raw: serde_json::Value) {
    if let Some(id) = exec_id { fail_execution(pool, events, id, "exceeded max deliveries").await; }
    let _ = jobs::dead_letter(pool, msg_id, &raw, "exceeded max deliveries").await;
}

/// Drive a durable run under a lease heartbeat: archive the message on a terminal
/// outcome, or leave the lease to expire and redeliver on a transient error.
fn spawn_workflow_run(
    pool: PgPool, registry: Arc<ToolRegistry>, events: WorkflowEvents, in_flight: InFlight,
    msg_id: i64, exec_id: uuid::Uuid, uid: uuid::Uuid, perms: Vec<String>, label: &'static str,
) {
    use crate::extensions::workflows::runner;
    // Single-flight: if a run task for this lease is still alive (e.g. the lease was
    // lost mid-run and the message redelivered), refuse to start a second one —
    // concurrent runs would re-invoke nodes and double-fire side effects.
    if !in_flight.lock().unwrap().insert(msg_id) {
        debug!(msg_id, %exec_id, "{label} workflow already in flight; skipping duplicate delivery");
        return;
    }
    tokio::spawn(async move {
        let _guard = InFlightGuard { set: in_flight, msg_id };
        let hb = runner::Heartbeat::lease(pool.clone(), msg_id);
        match runner::run(&registry, &pool, exec_id, uid, &perms, hb, &events).await {
            Ok(status) => {
                let _ = jobs::complete(&pool, msg_id).await;
                info!(msg_id, %exec_id, ?status, "{label} workflow finished");
            }
            Err(e) => warn!(msg_id, %exec_id, "{label} workflow run error (will retry): {e}"),
        }
    });
}

/// Triggered run (cron / record-change): resolve the workflow, gate via `fire_gate`
/// (run-as owner), then resume or snapshot an execution bound to this lease.
async fn dispatch_workflow_job(
    pool: &PgPool,
    tool_registry: &Arc<ToolRegistry>,
    msg_id: i64,
    read_ct: i32,
    workflow_id: &str,
    user_id: Option<uuid::Uuid>,
    trigger_data: serde_json::Value,
    events: &WorkflowEvents,
    in_flight: &InFlight,
    label: &'static str,
) {
    use crate::extensions::workflows::runner;

    let Ok(wf_uuid) = workflow_id.parse::<uuid::Uuid>() else {
        warn!(msg_id, workflow_id, "{label} workflow: invalid workflow_id");
        let _ = jobs::fail(pool, msg_id).await;
        return;
    };
    let app_id: String = match sqlx::query_scalar::<_, String>(
        "SELECT app_id FROM rootcx_system.workflows WHERE id = $1",
    ).bind(wf_uuid).fetch_optional(pool).await {
        Ok(Some(id)) => id,
        _ => {
            warn!(msg_id, workflow_id, "{label} workflow not found");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
    };

    if read_ct > MAX_DELIVERIES {
        warn!(msg_id, read_ct, %app_id, "{label} workflow exceeded max deliveries → dead-letter");
        let raw = serde_json::json!({"app_id": app_id, "workflow_id": workflow_id, "user_id": user_id});
        dead_letter_workflow(pool, events, msg_id, runner::inflight_for_lease(pool, msg_id).await, raw).await;
        return;
    }

    let uid = match crate::governance::triggers::fire_gate::assert_can_fire_workflow(pool, user_id, &app_id).await {
        Ok(uid) => uid,
        Err(denied) => {
            warn!(msg_id, %app_id, "{label} workflow denied: {denied}");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
    };
    let Some(perms) = perms_or_fail(pool, msg_id, uid).await else { return };

    // Resume the lease's in-flight execution (crash recovery) or snapshot a new one.
    let exec_id = match runner::inflight_for_lease(pool, msg_id).await {
        Some(id) => id,
        None => match runner::create_execution(pool, wf_uuid, &app_id, uid, Some(trigger_data), Some(msg_id)).await {
            Ok(id) => id,
            Err(e) => {
                warn!(msg_id, %app_id, "{label} workflow create failed: {e}");
                let _ = jobs::fail(pool, msg_id).await;
                return;
            }
        },
    };

    spawn_workflow_run(pool.clone(), Arc::clone(tool_registry), events.clone(), Arc::clone(in_flight), msg_id, exec_id, uid, perms, label);
}

/// Manual run: the execution row already exists (created by the HTTP handler, which
/// returned its id to the editor for streaming). Run-as caller — no `fire_gate`,
/// since the responsible human is the invoker. Resume by exec_id on redelivery.
async fn dispatch_manual_workflow(
    pool: &PgPool,
    tool_registry: &Arc<ToolRegistry>,
    events: &WorkflowEvents,
    in_flight: &InFlight,
    msg_id: i64,
    read_ct: i32,
    exec_id: uuid::Uuid,
    user_id: Option<uuid::Uuid>,
) {
    if read_ct > MAX_DELIVERIES {
        warn!(msg_id, read_ct, %exec_id, "manual workflow exceeded max deliveries → dead-letter");
        let raw = serde_json::json!({"manual": true, "execution_id": exec_id});
        dead_letter_workflow(pool, events, msg_id, Some(exec_id), raw).await;
        return;
    }

    // Denial is terminal: fail the (already-created) execution so it can't sit in
    // 'queued' forever, then drop the message.
    let Some(uid) = user_id else {
        warn!(msg_id, %exec_id, "manual workflow denied: no owner");
        fail_execution(pool, events, exec_id, "no responsible user").await;
        let _ = jobs::fail(pool, msg_id).await;
        return;
    };
    let Some(perms) = perms_or_fail(pool, msg_id, uid).await else {
        fail_execution(pool, events, exec_id, "permission resolution failed").await;
        return;
    };

    spawn_workflow_run(pool.clone(), Arc::clone(tool_registry), events.clone(), Arc::clone(in_flight), msg_id, exec_id, uid, perms, "manual");
}

pub fn spawn_scheduler(pool: PgPool, wm: Arc<WorkerManager>, tool_registry: Arc<ToolRegistry>, events: WorkflowEvents) -> SchedulerHandle {
    let wake = Arc::new(Notify::new());
    let cancel = CancellationToken::new();
    let w = Arc::clone(&wake);
    let c = cancel.clone();
    let in_flight: InFlight = Arc::new(Mutex::new(HashSet::new()));

    tokio::spawn(async move {
        info!("job scheduler started");
        loop {
            if c.is_cancelled() { break; }
            match jobs::read_next(&pool).await {
                Ok(Some((msg_id, read_ct, job_msg))) => {
                    let is_hook = job_msg.payload.get("_hook").and_then(|v| v.as_bool()) == Some(true);
                    let is_agent = job_msg.payload.get("action_type").and_then(|v| v.as_str()) == Some("agent");

                    let is_workflow = job_msg.payload.get("action_type").and_then(|v| v.as_str()) == Some("workflow");

                    // Manual run: execution already created by the HTTP handler.
                    if is_workflow && job_msg.payload.get("manual").and_then(|v| v.as_bool()) == Some(true) {
                        match job_msg.payload.get("execution_id").and_then(|v| v.as_str()).and_then(|s| s.parse().ok()) {
                            Some(exec_id) => dispatch_manual_workflow(&pool, &tool_registry, &events, &in_flight, msg_id, read_ct, exec_id, job_msg.user_id).await,
                            None => {
                                warn!(msg_id, "manual workflow: missing execution_id");
                                let _ = jobs::fail(&pool, msg_id).await;
                            }
                        }
                        continue;
                    }

                    if is_hook && is_agent {
                        let target_app = job_msg.payload.get("action_config")
                            .and_then(|c| c.get("app_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or(&job_msg.app_id)
                            .to_string();

                        let entity = job_msg.payload.get("entity").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let operation = job_msg.payload.get("operation").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let record = job_msg.payload.get("record").cloned().unwrap_or_default();
                        let message = format!("Entity event: {operation} on {entity}\n\nRecord:\n{record}");

                        dispatch_agent_job(&pool, &wm, msg_id, &target_app, message, job_msg.user_id, "hook").await;
                        continue;
                    }

                    if is_hook && is_workflow {
                        let wf_id = job_msg.payload.get("action_config")
                            .and_then(|c| c.get("workflow_id"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let trigger_data = serde_json::json!({
                            "entity": job_msg.payload.get("entity"),
                            "operation": job_msg.payload.get("operation"),
                            "record": job_msg.payload.get("record"),
                            "old_record": job_msg.payload.get("old_record"),
                        });
                        dispatch_workflow_job(&pool, &tool_registry, msg_id, read_ct, wf_id, job_msg.user_id, trigger_data, &events, &in_flight, "hook").await;
                        continue;
                    }

                    // Cron-triggered invocations
                    let is_cron = job_msg.payload.get("cron_id").is_some();
                    let cron_workflow_id = job_msg.payload.get("workflow_id").and_then(|v| v.as_str());

                    if let (true, Some(wf_id)) = (is_cron, cron_workflow_id) {
                        dispatch_workflow_job(&pool, &tool_registry, msg_id, read_ct, wf_id, job_msg.user_id, serde_json::json!({"trigger": "schedule"}), &events, &in_flight, "cron").await;
                        continue;
                    }

                    let is_cron_agent = if is_cron {
                        sqlx::query_scalar::<_, bool>(
                            "SELECT EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = $1)"
                        ).bind(&job_msg.app_id).fetch_one(&pool).await.unwrap_or(false)
                    } else { false };

                    if is_cron_agent {
                        let message = job_msg.payload.get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Scheduled invocation")
                            .to_string();

                        dispatch_agent_job(&pool, &wm, msg_id, &job_msg.app_id, message, job_msg.user_id, "cron").await;
                        continue;
                    }

                    // Regular job dispatch. Deny-by-default: a job with no owner
                    // (created_by NULL) has no responsible human, so RLS would
                    // see no identity. Refuse rather than fall back to admin.
                    let Some(uid) = job_msg.user_id else {
                        warn!(msg_id, app_id = %job_msg.app_id,
                            "job denied: no owner (created_by is NULL)");
                        let _ = jobs::fail(&pool, msg_id).await;
                        continue;
                    };
                    let caller = crate::principal::resolve_caller(&pool, uid).await;
                    if let Err(e) = wm.dispatch_job(&job_msg.app_id, msg_id.to_string(), job_msg.payload, caller).await {
                        warn!(msg_id, "dispatch failed: {e}");
                        let _ = jobs::fail(&pool, msg_id).await;
                    }
                    continue;
                }
                Ok(None) => {}
                Err(e) => error!("scheduler: {e}"),
            }
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = w.notified() => { debug!("scheduler woken"); }
                _ = c.cancelled() => { break; }
            }
        }
        info!("job scheduler stopped");
    });

    SchedulerHandle { wake, cancel }
}
