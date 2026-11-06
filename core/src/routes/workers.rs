use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool, pool_and_secrets, wm};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::governance::action_invocation::{Declaration, authorize, dispatch};
use crate::governance::authority::{require_admin, resolve_permissions, share_read_perms};
use crate::extensions::sharing::guard::{CallerAuth, authorize_public_rpc, find_public_rpc, find_public_rpc_full};
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
    _headers: HeaderMap,
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

    let p = pool(&rt);
    if let Some(decl) = find_public_rpc(&p, &app_id, &method).await?
        && !decl.publications.is_empty()
    {
        if !decl.scope.is_empty() {
            return Err(ApiError::Forbidden("scoped RPCs cannot use public publications".into()));
        }
        authorize_public_rpc(&decl, &auth, &app_id, &params)?;
        let execution = crate::governance::publications::resolve(&p, &app_id, &decl.publications).await?;
        let result = wm(&rt).rpc_public(&app_id, id, method, params, execution).await?;
        return Ok(Json(result));
    }
    let caller = match &auth {
        CallerAuth::User(identity) => {
            let declaration = Declaration::load(&p, &app_id, &method).await?;
            let (_, permissions) = resolve_permissions(&p, identity.user_id).await?;
            authorize(&app_id, &method, declaration.as_ref(), &permissions, false)?;
            let caller = RpcCaller::person(identity.user_id, &identity.email);
            return Ok(Json(dispatch(wm(&rt).action_caller().as_ref(), &app_id, &method, declaration.as_ref(), params, caller).await?));
        }
        CallerAuth::ShareToken(share) => {
            let (manifest, decl) = find_public_rpc_full(&p, &app_id, &method)
                .await?
                .ok_or_else(|| ApiError::Forbidden(format!("rpc '{method}' is not public")))?;
            authorize_public_rpc(&decl, &auth, &app_id, &params)?;

            let read_perms = share_read_perms(
                &p, &app_id, share.created_by, &manifest.data_contract,
            ).await?;
            RpcCaller {
                user_id: share.created_by.to_string(),
                email: String::new(),
                effective_perms: Some(read_perms),
                connection_id: None,
            }
        }
        CallerAuth::Anonymous => {
            let decl = find_public_rpc(&p, &app_id, &method)
                .await?
                .ok_or_else(|| ApiError::Unauthorized("missing or invalid authorization header".into()))?;
            authorize_public_rpc(&decl, &auth, &app_id, &params)?;
            RpcCaller {
                user_id: String::new(),
                email: String::new(),
                effective_perms: None,
                connection_id: None,
            }
        }
    };
    Ok(Json(wm(&rt).rpc(&app_id, id, method, params, Some(caller)).await?))
}
