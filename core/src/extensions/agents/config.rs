use std::path::Path;

use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::warn;

use crate::api_error::ApiError;
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
