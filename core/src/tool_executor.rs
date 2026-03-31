use std::sync::Arc;
use std::time::Instant;

use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::ipc::OutboundMessage;
use crate::tools::{AgentDispatcher, IntegrationCaller, Tool, ToolContext, check_permission};
use crate::worker::AgentEvent;

pub(crate) async fn execute(
    tool: Option<Arc<dyn Tool>>,
    tool_name: String,
    args: JsonValue,
    app_id: String,
    user_id: Uuid,
    invoker_user_id: Option<Uuid>,
    permissions: Vec<String>,
    pool: PgPool,
    agent_dispatch: Option<Arc<dyn AgentDispatcher>>,
    integration_caller: Option<Arc<dyn IntegrationCaller>>,
    out_tx: mpsc::Sender<OutboundMessage>,
    stream_tx: Option<mpsc::Sender<AgentEvent>>,
    invoke_id: String,
    call_id: String,
) {
    if let Err(e) = check_permission(&permissions, &format!("tool.{tool_name}")) {
        send_result(&out_tx, &stream_tx, &invoke_id, &call_id, &tool_name, None, Some(e), 0).await;
        return;
    }

    let start = Instant::now();
    let (result, err) = match tool {
        Some(t) => {
            let ctx = ToolContext { pool, app_id, user_id, invoker_user_id, permissions, args, agent_dispatch, integration_caller, stream_tx: stream_tx.clone() };
            match t.execute(&ctx).await {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(e)),
            }
        }
        None => (None, Some(format!("unknown tool: {tool_name}"))),
    };

    send_result(&out_tx, &stream_tx, &invoke_id, &call_id, &tool_name,
        result, err, start.elapsed().as_millis() as u64).await;
}

async fn send_result(
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
