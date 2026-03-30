use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;
use super::ToolContext;

#[derive(Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
}

pub async fn list_tools(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<ToolSummary>>, ApiError> {
    let tools = rt.tool_registry()
        .all_summaries()
        .into_iter()
        .map(|(name, description)| ToolSummary { name, description })
        .collect();
    Ok(Json(tools))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteToolRequest {
    pub app_id: String,
    pub args: JsonValue,
}

pub async fn execute_tool(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(tool_name): Path<String>,
    Json(body): Json<ExecuteToolRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let tool = rt.tool_registry().get(&tool_name)
        .ok_or_else(|| ApiError::NotFound(format!("unknown tool: '{tool_name}'")))?;
    let pool = rt.pool().clone();

    let (_, permissions) = crate::extensions::rbac::policy::resolve_permissions(&pool, &body.app_id, identity.user_id).await?;
    let result = tool.execute(&ToolContext {
        pool, app_id: body.app_id, user_id: identity.user_id, permissions, args: body.args,
        agent_dispatch: None, stream_tx: None,
    }).await.map_err(ApiError::Internal)?;
    Ok(Json(result))
}
