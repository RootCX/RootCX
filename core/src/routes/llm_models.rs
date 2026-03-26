use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::FromRow;

use super::{SharedRuntime, pool};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct LlmModel {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub config: JsonValue,
    #[serde(default)]
    pub is_default: bool,
}

async fn clear_default(pool: &sqlx::PgPool) -> Result<(), ApiError> {
    sqlx::query("UPDATE rootcx_system.llm_models SET is_default = FALSE WHERE is_default = TRUE")
        .execute(pool).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(())
}

pub async fn list_llm_models(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<LlmModel>>, ApiError> {
    let pool = pool(&rt).await?;
    let rows: Vec<LlmModel> = sqlx::query_as(
        "SELECT id, name, provider, model, config, is_default FROM rootcx_system.llm_models ORDER BY created_at",
    )
    .fetch_all(&pool).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(rows))
}

pub async fn create_llm_model(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Json(input): Json<LlmModel>,
) -> Result<(StatusCode, Json<LlmModel>), ApiError> {
    let pool = pool(&rt).await?;
    if input.is_default { clear_default(&pool).await?; }

    sqlx::query(
        "INSERT INTO rootcx_system.llm_models (id, name, provider, model, config, is_default) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&input.id).bind(&input.name).bind(&input.provider)
    .bind(&input.model).bind(&input.config).bind(input.is_default)
    .execute(&pool).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(input)))
}

pub async fn update_llm_model(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<String>,
    Json(input): Json<LlmModel>,
) -> Result<StatusCode, ApiError> {
    let pool = pool(&rt).await?;
    if input.is_default { clear_default(&pool).await?; }

    let result = sqlx::query(
        "UPDATE rootcx_system.llm_models SET name = $2, provider = $3, model = $4, config = $5, is_default = $6 WHERE id = $1",
    )
    .bind(&id).bind(&input.name).bind(&input.provider)
    .bind(&input.model).bind(&input.config).bind(input.is_default)
    .execute(&pool).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    if result.rows_affected() == 0 { return Err(ApiError::NotFound(format!("LLM model '{id}' not found"))); }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_llm_model(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let pool = pool(&rt).await?;
    let result = sqlx::query("DELETE FROM rootcx_system.llm_models WHERE id = $1")
        .bind(&id).execute(&pool).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    if result.rows_affected() == 0 { return Err(ApiError::NotFound(format!("LLM model '{id}' not found"))); }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_default_llm_model(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let pool = pool(&rt).await?;
    let result = sqlx::query("UPDATE rootcx_system.llm_models SET is_default = (id = $1) WHERE id = $1 OR is_default = TRUE")
        .bind(&id).execute(&pool).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    if result.rows_affected() == 0 { return Err(ApiError::NotFound(format!("LLM model '{id}' not found"))); }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn fetch_default_llm(pool: &sqlx::PgPool) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT provider, model FROM rootcx_system.llm_models ORDER BY is_default DESC, created_at ASC LIMIT 1",
    )
    .fetch_optional(pool).await
}

pub async fn get_forge_model(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT provider, model FROM rootcx_system.llm_models WHERE is_default = TRUE LIMIT 1",
    )
    .fetch_optional(&pool).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    let model_str = match row {
        Some((ref provider, ref model)) if provider == "bedrock" => format!("amazon-bedrock/{model}"),
        Some((provider, model)) => format!("{provider}/{model}"),
        None => "anthropic/claude-sonnet-4-6".to_string(),
    };
    Ok(Json(serde_json::json!({ "model": model_str })))
}
