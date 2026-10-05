use std::sync::Arc;

use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tokio::sync::mpsc;
use crate::governance::execution::AgentAuthority;

use crate::ipc::OutboundMessage;
use crate::tools::{ActionCaller, AgentDispatcher, DispatchError, IntegrationCaller, Tool, ToolContext};
use crate::worker::AgentEvent;

pub(crate) async fn execute(
    tool: Option<Arc<dyn Tool>>,
    tool_name: String,
    args: JsonValue,
    authority: AgentAuthority,
    pool: PgPool,
    agent_dispatch: Option<Arc<dyn AgentDispatcher>>,
    integration_caller: Option<Arc<dyn IntegrationCaller>>,
    action_caller: Option<Arc<dyn ActionCaller>>,
    out_tx: mpsc::Sender<OutboundMessage>,
    stream_tx: Option<mpsc::Sender<AgentEvent>>,
    invoke_id: String,
    call_id: String,
) {
    let permissions = match authority.revalidate(&pool).await {
        Ok(permissions) => permissions,
        Err(error) => {
            send_result(&out_tx, &stream_tx, &invoke_id, &call_id, &tool_name, None, Some(error), 0).await;
            return;
        }
    };
    let app_id = authority.app().to_string();
    let ctx = ToolContext {
        user_id: crate::extensions::agents::agent_user_id(&app_id),
        invoker_user_id: Some(authority.human()), task_scope: authority.task_scope(),
        authority: Some(authority),
        pool, core_bound_app_id: Some(app_id.clone()), app_id, permissions, args,
        agent_dispatch, integration_caller, action_caller, stream_tx: stream_tx.clone(),
        idempotency_key: None,
    };

    let (result, error, duration_ms) = match tool {
        Some(t) => {
            let outcome = crate::tools::dispatch(&tool_name, t, &ctx).await;
            match outcome.value {
                Ok(v) => (Some(v), None, outcome.duration_ms),
                Err(DispatchError::PermissionDenied(e) | DispatchError::ExecutionFailed(e)) =>
                    (None, Some(e), outcome.duration_ms),
            }
        }
        None => (None, Some(format!("unknown tool: {tool_name}")), 0),
    };

    send_result(&out_tx, &stream_tx, &invoke_id, &call_id, &tool_name,
        result, error, duration_ms).await;
}

pub(crate) async fn send_result(
    out_tx: &mpsc::Sender<OutboundMessage>,
    stream_tx: &Option<mpsc::Sender<AgentEvent>>,
    invoke_id: &str, call_id: &str, tool_name: &str,
    result: Option<JsonValue>, error: Option<String>, duration_ms: u64,
) {
    let _ = out_tx.send(OutboundMessage::AgentToolResult {
        invoke_id: invoke_id.into(), call_id: call_id.into(),
        result: result.clone(), error: error.clone(),
    }).await;
    if let Some(tx) = stream_tx {
        let _ = tx.send(AgentEvent::ToolCallCompleted {
            call_id: call_id.into(), tool_name: tool_name.into(),
            output: result, error, duration_ms,
        }).await;
    }
}
