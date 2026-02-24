use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use rootcx_shared_types::AiConfig;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;

async fn load_ai_config(pool: &sqlx::PgPool) -> Result<Option<AiConfig>, ApiError> {
    let value: Option<JsonValue> = sqlx::query_scalar(
        "SELECT value FROM rootcx_system.config WHERE key = 'ai'",
    )
    .fetch_optional(pool)
    .await?;
    value
        .map(|v| serde_json::from_value(v).map_err(|e| ApiError::Internal(e.to_string())))
        .transpose()
}

pub async fn get_ai_config(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<AiConfig>, ApiError> {
    let pool = pool(&rt).await?;
    load_ai_config(&pool)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound("AI not configured".into()))
}

pub async fn set_ai_config(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Json(config): Json<AiConfig>,
) -> Result<StatusCode, ApiError> {
    let pool = pool(&rt).await?;
    let value = serde_json::to_value(&config).map_err(|e| ApiError::Internal(e.to_string()))?;
    sqlx::query(
        "INSERT INTO rootcx_system.config (key, value) VALUES ('ai', $1)
         ON CONFLICT (key) DO UPDATE SET value = $1",
    )
    .bind(&value)
    .execute(&pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn get_forge_config(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    let config = load_ai_config(&pool).await?.unwrap_or_default();
    Ok(Json(json!({ "model": config.forge_model_string() })))
}
