use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool, pool_and_secrets, wm};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::require_admin;
use crate::ipc::RpcCaller;

pub async fn start_worker(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let (p, secrets) = pool_and_secrets(&rt).await?;
    require_admin(&p, "core", identity.user_id).await?;
    let w = wm(&rt).await?;
    w.start_app(&p, &secrets, &app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' started", app_id) })))
}

pub async fn stop_worker(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let p = pool(&rt).await?;
    require_admin(&p, "core", identity.user_id).await?;
    wm(&rt).await?.stop_app(&app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' stopped", app_id) })))
}

pub async fn worker_status(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let s = wm(&rt).await?.worker_status(&app_id).await?;
    Ok(Json(json!({ "app_id": app_id, "status": s })))
}

pub async fn all_worker_statuses(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<JsonValue>, ApiError> {
    let p = pool(&rt).await?;
    require_admin(&p, "core", identity.user_id).await?;
    Ok(Json(json!({ "workers": wm(&rt).await?.all_statuses().await })))
}

pub async fn rpc_proxy(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let method = body
        .get("method")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("missing 'method'".into()))?
        .to_string();
    let params = body.get("params").cloned().unwrap_or(json!({}));
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let caller = if !identity.user_id.is_nil() {
        let raw_token = headers.get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer ").map(String::from));
        Some(RpcCaller { user_id: identity.user_id.to_string(), username: identity.username, auth_token: raw_token })
    } else {
        None
    };

    Ok(Json(wm(&rt).await?.rpc(&app_id, id, method, params, caller).await?))
}
