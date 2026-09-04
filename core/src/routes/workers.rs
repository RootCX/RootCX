use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool, pool_and_secrets, wm};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::governance::authority::{has_permission_db, require_admin, share_read_perms};
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
            // `None` when the method is not a declared action; `Some(isolated)`
            // when it is. Two different questions, one lookup: declaration
            // decides which permission may authorize the call, isolation decides
            // whether Core poses an invocation identity in SQL for it.
            let declaration: Option<bool> = sqlx::query_scalar(
                "SELECT COALESCE((action->>'isolatedScope')::boolean, false)
                   FROM rootcx_system.apps app
                   CROSS JOIN LATERAL jsonb_array_elements(
                     COALESCE(app.manifest->'actions', '[]'::jsonb)
                   ) action
                  WHERE app.id = $1
                    AND action->>'id' = $2",
            )
            .bind(&app_id)
            .bind(&method)
            .fetch_optional(&p)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
            let declared_action = declaration.is_some();
            // Two grains of one right, the coarse implying the fine: `invoke` is
            // the whole app, `action:{method}` is that method alone. Same idiom as
            // `app:{id}:*` implying everything beneath it. Requiring BOTH would
            // make `invoke` meaningless alone and revoke every grant issued before
            // the fine keys existed; a role is narrowed by holding the fine keys
            // INSTEAD of `invoke`.
            let invoke_key = format!("app:{app_id}:invoke");
            let action_key = format!("app:{app_id}:action:{method}");
            let allowed = has_permission_db(&p, identity.user_id, &invoke_key).await?
                || (declared_action
                    && has_permission_db(&p, identity.user_id, &action_key).await?);
            if !allowed {
                let needed = if declared_action {
                    format!("{invoke_key} or {action_key}")
                } else {
                    invoke_key
                };
                return Err(ApiError::Forbidden(format!("permission denied: {needed}")));
            }
            (Some(RpcCaller {
                user_id: identity.user_id.to_string(),
                email: identity.email.clone(),
                effective_perms: None,
                connection_id: None,
            // A scope means a dedicated process for the life of the call, so it
            // is posed only for an action that asked for one. Every other
            // declared action shares its caller's worker and reaches SQL with
            // the invocation settings empty — nothing to borrow, and a policy
            // written against them denies rather than trusting a neighbour.
            }), (declaration == Some(true)).then(|| method.clone()))
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
