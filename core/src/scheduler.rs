use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::extensions::agents::persistence;
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

async fn dispatch_workflow_job(
    pool: &PgPool,
    tool_registry: &Arc<ToolRegistry>,
    msg_id: i64,
    workflow_id: &str,
    user_id: Option<uuid::Uuid>,
    trigger_data: serde_json::Value,
    label: &'static str,
) {
    let wf_uuid = match workflow_id.parse::<uuid::Uuid>() {
        Ok(id) => id,
        Err(_) => {
            warn!(msg_id, workflow_id, "{label} workflow: invalid workflow_id");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
    };

    // Resolve the workflow's backing app_id
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

    let uid = match crate::governance::triggers::fire_gate::assert_can_fire_workflow(
        pool, user_id, &app_id,
    ).await {
        Ok(uid) => uid,
        Err(denied) => {
            warn!(msg_id, %app_id, "{label} workflow denied: {denied}");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
    };

    let (_, perms) = match crate::governance::authority::resolve_permissions(pool, uid).await {
        Ok(p) => p,
        Err(e) => {
            warn!(msg_id, %app_id, "{label} workflow perms: {e:?}");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
    };

    let pool = pool.clone();
    let registry = Arc::clone(tool_registry);
    let wf_id_str = workflow_id.to_string();

    tokio::spawn(async move {
        match crate::extensions::workflows::executor::run_workflow(
            &registry, &pool, &app_id, wf_uuid, uid, &perms, Some(trigger_data),
        ).await {
            Ok((exec_id, _)) => {
                let _ = jobs::complete(&pool, msg_id).await;
                info!(msg_id, %app_id, workflow_id = %wf_id_str, %exec_id, "{label} workflow completed");
            }
            Err(e) => {
                warn!(msg_id, %app_id, workflow_id = %wf_id_str, "{label} workflow failed: {e}");
                let _ = jobs::fail(&pool, msg_id).await;
            }
        }
    });
}

pub fn spawn_scheduler(pool: PgPool, wm: Arc<WorkerManager>, tool_registry: Arc<ToolRegistry>) -> SchedulerHandle {
    let wake = Arc::new(Notify::new());
    let cancel = CancellationToken::new();
    let w = Arc::clone(&wake);
    let c = cancel.clone();

    tokio::spawn(async move {
        info!("job scheduler started");
        loop {
            if c.is_cancelled() { break; }
            match jobs::read_next(&pool).await {
                Ok(Some((msg_id, job_msg))) => {
                    let is_hook = job_msg.payload.get("_hook").and_then(|v| v.as_bool()) == Some(true);
                    let is_agent = job_msg.payload.get("action_type").and_then(|v| v.as_str()) == Some("agent");

                    let is_workflow = job_msg.payload.get("action_type").and_then(|v| v.as_str()) == Some("workflow");

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
                        dispatch_workflow_job(&pool, &tool_registry, msg_id, wf_id, job_msg.user_id, trigger_data, "hook").await;
                        continue;
                    }

                    // Cron-triggered invocations
                    let is_cron = job_msg.payload.get("cron_id").is_some();
                    let cron_workflow_id = job_msg.payload.get("workflow_id").and_then(|v| v.as_str());

                    if let (true, Some(wf_id)) = (is_cron, cron_workflow_id) {
                        dispatch_workflow_job(&pool, &tool_registry, msg_id, wf_id, job_msg.user_id, serde_json::json!({"trigger": "schedule"}), "cron").await;
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
