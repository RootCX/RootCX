use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool, pool_and_secrets, wm};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::require_admin;
use crate::extensions::sharing::guard::{CallerAuth, authorize_public_rpc, find_public_rpc};
use crate::ipc::RpcCaller;

pub async fn start_worker(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let (p, secrets) = pool_and_secrets(&rt);
    require_admin(&p, identity.user_id).await?;
    let w = wm(&rt);
    w.start_app(&p, &secrets, &app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' started", app_id) })))
}

pub async fn stop_worker(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let p = pool(&rt);
    require_admin(&p, identity.user_id).await?;
    wm(&rt).stop_app(&app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' stopped", app_id) })))
}

pub async fn worker_status(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let s = wm(&rt).worker_status(&app_id).await?;
    Ok(Json(json!({ "app_id": app_id, "status": s })))
}

pub async fn all_worker_statuses(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<JsonValue>, ApiError> {
    let p = pool(&rt);
    require_admin(&p, identity.user_id).await?;
    Ok(Json(json!({ "workers": wm(&rt).all_statuses().await })))
}

pub async fn rpc_proxy(
    auth: CallerAuth,
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

    let raw_token = headers.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer ").map(String::from));

    let caller = match &auth {
        CallerAuth::User(identity) => Some(RpcCaller {
            user_id: identity.user_id.to_string(),
            email: identity.email.clone(),
            auth_token: raw_token,
        }),
        CallerAuth::ShareToken(_) | CallerAuth::Anonymous => {
            // Anonymous / share-scoped call. Require explicit opt-in via
            // the app's `public` manifest. Authed callers (User) keep the
            // normal flow — the worker layer enforces RBAC.
            let pool = pool(&rt);
            let decl = find_public_rpc(&pool, &app_id, &method).await?.ok_or_else(|| {
                if matches!(auth, CallerAuth::Anonymous) {
                    ApiError::Unauthorized("missing or invalid authorization header".into())
                } else {
                    ApiError::Forbidden(format!("rpc '{method}' is not public"))
                }
            })?;

            authorize_public_rpc(&decl, &auth, &app_id, &params)?;

            // Pass an anonymous caller down to the worker. The share token
            // (if any) is forwarded so the RPC code can still inspect it
            // via the SDK's request context if it wants extra checks.
            Some(RpcCaller {
                user_id: String::new(),
                email: String::new(),
                auth_token: raw_token,
            })
        }
    };

    Ok(Json(wm(&rt).rpc(&app_id, id, method, params, caller).await?))
}
