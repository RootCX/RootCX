use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool, pool_and_secrets, wm};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::governance::authority::{require_admin, share_read_perms};
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
    let (caller, action_scope) = match &auth {
        CallerAuth::User(identity) => {
            if !crate::governance::authority::has_permission_db(
                &p, identity.user_id, &format!("app:{app_id}:invoke"),
            ).await? {
                return Err(ApiError::Forbidden(format!("permission denied: app:{app_id}:invoke")));
            }
            let declared_action: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                   SELECT 1
                     FROM rootcx_system.apps app
                     CROSS JOIN LATERAL jsonb_array_elements(
                       COALESCE(app.manifest->'actions', '[]'::jsonb)
                     ) action
                    WHERE app.id = $1
                      AND action->>'id' = $2
                 )",
            )
            .bind(&app_id)
            .bind(&method)
            .fetch_one(&p)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
            if declared_action && !crate::governance::authority::has_permission_db(
                &p, identity.user_id, &format!("app:{app_id}:action:{method}"),
            ).await? {
                return Err(ApiError::Forbidden(format!(
                    "permission denied: app:{app_id}:action:{method}"
                )));
            }
            (Some(RpcCaller {
                user_id: identity.user_id.to_string(),
                email: identity.email.clone(),
                effective_perms: None,
                connection_id: None,
            }), declared_action.then(|| method.clone()))
        }
        CallerAuth::ShareToken(share) => {
            let (manifest, decl) = find_public_rpc_full(&p, &app_id, &method)
                .await?
                .ok_or_else(|| ApiError::Forbidden(format!("rpc '{method}' is not public")))?;
            authorize_public_rpc(&decl, &auth, &app_id, &params)?;

            let read_perms = share_read_perms(
                &p, &app_id, share.created_by, &manifest.data_contract,
            ).await?;
            (Some(RpcCaller {
                user_id: share.created_by.to_string(),
                email: String::new(),
                effective_perms: Some(read_perms),
                connection_id: None,
            }), None)
        }
        CallerAuth::Anonymous => {
            let decl = find_public_rpc(&p, &app_id, &method)
                .await?
                .ok_or_else(|| ApiError::Unauthorized("missing or invalid authorization header".into()))?;
            authorize_public_rpc(&decl, &auth, &app_id, &params)?;
            (Some(RpcCaller {
                user_id: String::new(),
                email: String::new(),
                effective_perms: None,
                connection_id: None,
            }), None)
        }
    };

    let result = match action_scope {
        Some(action) => wm(&rt).rpc_action(&app_id, id, action, params, caller).await?,
        None => wm(&rt).rpc(&app_id, id, method, params, caller).await?,
    };
    Ok(Json(result))
}
