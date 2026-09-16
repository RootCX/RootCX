use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;
use super::{DispatchError, ToolContext};

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
        .filter(|(name, _)| rt.tool_registry().get(name).is_some_and(|tool| !tool.requires_agent_context()))
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

fn map_tool_execution_error(error: String) -> ApiError {
    // The generic HTTP route is never Core-bound. This particular failure is a
    // policy rejection, not a server fault; keeping the mapping here makes the
    // boundary explicit and testable without booting PostgreSQL.
    if error == "cross-app access requires a Core-bound application worker" {
        ApiError::Forbidden(error)
    } else {
        ApiError::Internal(error)
    }
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

    let (_, permissions) = crate::governance::authority::resolve_permissions(&pool, identity.user_id).await?;
    if tool.requires_agent_context() {
        let permission = format!("tool:{tool_name}");
        if !crate::governance::authority::has_permission(&permissions, &permission) {
            return Err(ApiError::Forbidden(permission));
        }
        return Err(ApiError::BadRequest(format!(
            "tool '{tool_name}' requires agent execution context and is unavailable through generic HTTP tool execution"
        )));
    }
    let ctx = ToolContext {
        pool, core_bound_app_id: None, app_id: body.app_id, user_id: identity.user_id, invoker_user_id: None,
        permissions, task_scope: None, args: body.args,
        agent_dispatch: None, integration_caller: None, action_caller: None, stream_tx: None,
        idempotency_key: None,
    };
    let outcome = super::dispatch(&tool_name, tool, &ctx).await;
    match outcome.value {
        Ok(v) => Ok(Json(v)),
        Err(DispatchError::PermissionDenied(e)) => Err(ApiError::Forbidden(e)),
        Err(DispatchError::ExecutionFailed(e)) => Err(map_tool_execution_error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn an_http_cross_app_identity_attempt_is_a_forbidden_policy_error() {
        assert_eq!(
            map_tool_execution_error(
                "cross-app access requires a Core-bound application worker".into()
            )
            .into_response()
            .status(),
            axum::http::StatusCode::FORBIDDEN,
        );
        assert_eq!(
            map_tool_execution_error("database exploded".into())
                .into_response()
                .status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        );
    }
}
