use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;

use crate::api_error::ApiError;
use crate::secrets::SecretManager;

pub(crate) async fn build_agent_config(
    pool: &PgPool, sm: &SecretManager, app_id: &str,
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

    let mut provider = config.get("provider").cloned().unwrap_or(json!(null));
    resolve_secret_refs(pool, sm, &mut provider).await?;

    Ok(json!({
        "provider": provider,
        "limits": config.get("limits"),
        "_appId": app_id,
        "_toolDescriptors": tool_descriptors,
        "_graphPath": config.get("graph"),
        "_supervision": config.get("supervision"),
    }))
}

async fn resolve_secret_refs(pool: &PgPool, sm: &SecretManager, value: &mut JsonValue) -> Result<(), ApiError> {
    let Some(obj) = value.as_object() else { return Ok(()) };
    let mut resolved = Vec::new();
    for (k, v) in obj {
        let Some(raw) = v.as_str() else { continue };
        if let Some(key) = raw.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            let secret = sm.get(pool, "_platform", key).await
                .map_err(|e| ApiError::Internal(format!("secret resolution failed: {e}")))?
                .ok_or_else(|| ApiError::BadRequest(format!("platform secret '{key}' not found")))?;
            resolved.push((k.clone(), secret));
        }
    }
    let obj = value.as_object_mut().unwrap();
    for (k, secret) in resolved { obj.insert(k, JsonValue::String(secret)); }
    Ok(())
}

pub(crate) async fn load_system_prompt(
    app_id: &str, config: &JsonValue, data_dir: &std::path::Path,
) -> Result<String, ApiError> {
    let prompt_path = config.get("systemPrompt").and_then(|p| p.as_str()).unwrap_or("./agent/system.md");
    let rel = prompt_path.strip_prefix("./").unwrap_or(prompt_path);
    let full_path = data_dir.join("apps").join(app_id).join(rel);
    tokio::fs::read_to_string(&full_path).await.map_err(|e| {
        ApiError::Internal(format!("failed to read system prompt at {}: {e}", full_path.display()))
    })
}

pub(crate) async fn load_history(
    pool: &PgPool, memory_enabled: bool, session_id: &str,
) -> Result<Vec<JsonValue>, ApiError> {
    if !memory_enabled { return Ok(vec![]); }

    let messages: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT role, content FROM rootcx_system.agent_messages
         WHERE session_id = $1::uuid ORDER BY created_at ASC",
    ).bind(session_id).fetch_all(pool).await?;

    if !messages.is_empty() {
        return Ok(messages.into_iter().map(|(role, content)| {
            json!({"role": role, "content": content.unwrap_or_default()})
        }).collect());
    }

    // Fallback: legacy JSONB messages column
    match sqlx::query_scalar::<_, JsonValue>(
        "SELECT messages FROM rootcx_system.agent_sessions WHERE id = $1::uuid",
    ).bind(session_id).fetch_optional(pool).await? {
        None => Ok(vec![]),
        Some(v) => v.as_array().cloned()
            .ok_or_else(|| ApiError::Internal("agent session messages is not a JSON array".into())),
    }
}
