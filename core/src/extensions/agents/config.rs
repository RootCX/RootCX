use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::warn;

use crate::api_error::ApiError;
use crate::auth::AuthConfig;
use crate::ipc::AgentInvokePayload;
use crate::tools::ToolRegistry;
use rootcx_types::AgentDefinition;

pub(crate) async fn load_agent_json(app_dir: &Path) -> Option<AgentDefinition> {
    let path = app_dir.join("agent.json");
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => return None, // no agent.json — not an agent app
    };
    match serde_json::from_slice(&bytes) {
        Ok(def) => Some(def),
        Err(e) => {
            warn!(path = %path.display(), "invalid agent.json: {e}");
            None
        }
    }
}

pub(crate) async fn build_agent_config(
    pool: &PgPool, app_id: &str,
    config: &JsonValue, tool_registry: &crate::tools::ToolRegistry,
) -> Result<JsonValue, ApiError> {
    let data_contract: JsonValue = sqlx::query_scalar(
        "SELECT COALESCE(manifest->'dataContract', '[]'::jsonb) FROM rootcx_system.apps WHERE id = $1",
    )
    .bind(app_id).fetch_optional(pool).await?
    .ok_or_else(|| ApiError::NotFound(format!("app '{app_id}' not found")))?;

    let agent_uid = super::agent_user_id(app_id);
    let (_, perms) = crate::extensions::rbac::policy::resolve_permissions(pool, app_id, agent_uid).await?;
    let tool_descriptors = tool_registry.descriptors_for_permissions(&perms, &data_contract);

    Ok(json!({
        "limits": config.get("limits"),
        "_appId": app_id,
        "_toolDescriptors": tool_descriptors,
        "_supervision": config.get("supervision"),
    }))
}

pub(crate) async fn load_system_prompt(
    app_id: &str, config: &JsonValue, data_dir: &std::path::Path,
) -> Result<String, ApiError> {
    let prompt_path = config.get("systemPrompt").and_then(|p| p.as_str()).unwrap_or("./agent/system.md");
    let rel = prompt_path.strip_prefix("./").unwrap_or(prompt_path);

    let rel_path = std::path::Path::new(rel);
    if rel_path.is_absolute() || rel_path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(ApiError::BadRequest(format!("systemPrompt path '{rel}' contains forbidden traversal")));
    }

    let app_root = data_dir.join("apps").join(app_id);
    let full_path = app_root.join(rel);
    let canonical = std::fs::canonicalize(&full_path).map_err(|e| {
        ApiError::Internal(format!("failed to resolve system prompt at {}: {e}", full_path.display()))
    })?;
    if !canonical.starts_with(&app_root) {
        return Err(ApiError::BadRequest("systemPrompt path escapes app directory".into()));
    }
    tokio::fs::read_to_string(&canonical).await.map_err(|e| {
        ApiError::Internal(format!("failed to read system prompt at {}: {e}", canonical.display()))
    })
}

pub(crate) async fn load_history(
    pool: &PgPool, memory_enabled: bool, app_id: &str, session_id: &str,
) -> Result<Vec<JsonValue>, ApiError> {
    if !memory_enabled { return Ok(vec![]); }

    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.agent_sessions WHERE id = $1::uuid AND app_id = $2)",
    ).bind(session_id).bind(app_id).fetch_one(pool).await?;
    if !exists { return Ok(vec![]); }

    let messages: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT role, content FROM rootcx_system.agent_messages
         WHERE session_id = $1::uuid ORDER BY created_at ASC",
    ).bind(session_id).fetch_all(pool).await?;

    Ok(messages.into_iter().map(|(role, content)| {
        json!({"role": role, "content": content.unwrap_or_default()})
    }).collect())
}

/// Build a complete invoke payload for an agent. Shared by HTTP route and internal orchestrator.
pub(crate) async fn build_invoke_payload(
    pool: &PgPool,
    app_id: &str,
    message: String,
    session_id: String,
    history: Vec<JsonValue>,
    data_dir: &Path,
    auth_config: &Arc<AuthConfig>,
    tool_registry: &ToolRegistry,
) -> Result<AgentInvokePayload, String> {
    let agent_config_json: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
    )
    .bind(app_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| format!("no agent for app '{app_id}'"))?;

    let system_prompt = load_system_prompt(app_id, &agent_config_json, data_dir)
        .await.map_err(|e| format!("{e:?}"))?;
    let agent_config = build_agent_config(pool, app_id, &agent_config_json, tool_registry)
        .await.map_err(|e| format!("{e:?}"))?;

    let agent_uid = super::agent_user_id(app_id);
    let agent_token = crate::auth::jwt::encode_access(auth_config, agent_uid, &format!("agent:{app_id}"))
        .map_err(|e| e.to_string())?;

    Ok(AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id,
        message,
        system_prompt,
        config: agent_config,
        auth_token: agent_token,
        history,
        caller: None,
    })
}
