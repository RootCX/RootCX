use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value as JsonValue};

use crate::api_error::ApiError;
use crate::auth::{jwt, AuthConfig};
use crate::ipc::RpcCaller;
use super::{SharedRuntime, pool_and_secrets, wm};

pub async fn start_worker(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = pool_and_secrets(&rt).await?;
    let w = wm(&rt).await?;
    w.start_app(&pool, &secrets, &app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' started", app_id) })))
}

pub async fn stop_worker(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<JsonValue>, ApiError> {
    wm(&rt).await?.stop_app(&app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' stopped", app_id) })))
}

pub async fn worker_status(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<JsonValue>, ApiError> {
    let s = wm(&rt).await?.worker_status(&app_id).await?;
    Ok(Json(json!({ "app_id": app_id, "status": s })))
}

pub async fn all_worker_statuses(State(rt): State<SharedRuntime>) -> Result<Json<JsonValue>, ApiError> {
    Ok(Json(json!({ "workers": wm(&rt).await?.all_statuses().await })))
}

pub async fn rpc_proxy(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let method = body.get("method").and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("missing 'method'".into()))?.to_string();
    let params = body.get("params").cloned().unwrap_or(json!({}));
    let id = body.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
        .map(String::from).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let caller = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .and_then(|token| jwt::decode(&auth_config, token).ok())
        .filter(|c| !c.username.is_empty())
        .map(|c| RpcCaller { user_id: c.sub, username: c.username });

    Ok(Json(wm(&rt).await?.rpc(&app_id, id, method, params, caller).await?))
}
