use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::extensions::agents::persistence;
use crate::ipc::{AgentInvokePayload, LlmModelRef, RpcCaller};
use crate::{jobs, worker_manager::WorkerManager};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct SchedulerHandle {
    pub wake: Arc<Notify>,
    pub cancel: CancellationToken,
}

async fn resolve_caller(pool: &PgPool, user_id: uuid::Uuid) -> Option<RpcCaller> {
    let (email,): (String,) = sqlx::query_as("SELECT email FROM rootcx_system.users WHERE id = $1")
        .bind(user_id).fetch_optional(pool).await.ok()??;
    Some(RpcCaller { user_id: user_id.to_string(), email, effective_perms: None })
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
    // Deny-by-default: no delegator = no authority
    if invoker_user_id.is_none() {
        error!(msg_id, app_id = %target_app,
            "{label} agent denied: trigger has no owner (created_by is NULL). \
             Re-deploy the app or assign an owner to restore automatic execution.");
        let _ = jobs::fail(pool, msg_id).await;
        return;
    }

    // Validate standing mandate (invoker_user_id guaranteed Some after early-return above)
    let delegator = invoker_user_id.unwrap();
    let agent_uid = crate::extensions::agents::agent_user_id(target_app);
    match crate::delegations::is_valid(pool, delegator, agent_uid).await {
        Ok(true) => {}
        Ok(false) => {
            warn!(msg_id, app_id = %target_app, "no valid delegation for {label} agent");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
        Err(e) => {
            warn!(msg_id, app_id = %target_app, "delegation check failed: {e}");
            let _ = jobs::fail(pool, msg_id).await;
            return;
        }
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
    };

    let system_user = uuid::Uuid::nil();
    let _ = persistence::ensure_session(pool, &session_id, target_app, system_user).await;
    let _ = persistence::persist_message(pool, &session_id, "user", &user_message, None, false).await;

    match wm.agent_invoke(target_app, invoke_payload).await {
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

pub fn spawn_scheduler(pool: PgPool, wm: Arc<WorkerManager>) -> SchedulerHandle {
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

                    // Cron-triggered agent invocation
                    let is_cron = job_msg.payload.get("cron_id").is_some();
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
                    let caller = resolve_caller(&pool, uid).await;
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
