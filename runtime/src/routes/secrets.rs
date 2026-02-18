use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value as JsonValue};

use crate::api_error::ApiError;
use super::{SharedRuntime, pool_and_secrets};

pub async fn set_secret(State(rt): State<SharedRuntime>, Path(app_id): Path<String>, Json(body): Json<JsonValue>) -> Result<Json<JsonValue>, ApiError> {
    let key = body.get("key").and_then(|v| v.as_str()).ok_or_else(|| ApiError::BadRequest("missing 'key'".into()))?;
    let val = body.get("value").and_then(|v| v.as_str()).ok_or_else(|| ApiError::BadRequest("missing 'value'".into()))?;
    let (pool, sm) = pool_and_secrets(&rt).await?;
    sm.set(&pool, &app_id, key, val).await?;
    Ok(Json(json!({ "message": format!("secret '{key}' set") })))
}

pub async fn delete_secret(State(rt): State<SharedRuntime>, Path((app_id, key)): Path<(String, String)>) -> Result<Json<JsonValue>, ApiError> {
    let (pool, sm) = pool_and_secrets(&rt).await?;
    if sm.delete(&pool, &app_id, &key).await? {
        Ok(Json(json!({ "message": format!("secret '{key}' deleted") })))
    } else {
        Err(ApiError::NotFound(format!("secret '{key}' not found")))
    }
}

pub async fn list_secrets(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<Vec<String>>, ApiError> {
    let (pool, sm) = pool_and_secrets(&rt).await?;
    Ok(Json(sm.list_keys(&pool, &app_id).await?))
}
