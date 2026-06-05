//! Route guard for the public surface.
//!
//! Resolves the request's auth situation into one of three states:
//!
//! - **User**: a valid JWT was presented → behave like the existing handlers.
//! - **ShareToken**: a valid share token was presented → carry the resolved
//!   share's `context` through to the handler so it can enforce scope match.
//! - **Anonymous**: no Authorization header at all → handler must check the
//!   app manifest to decide whether to allow the call.
//!
//! Bad credentials (a Bearer that's neither a valid JWT nor a known share
//! token) always 401.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde_json::Value as JsonValue;
use sqlx::PgPool;

use rootcx_types::{AppManifest, PublicRpc};

use crate::api_error::ApiError;
use crate::auth::AuthConfig;
use crate::auth::identity::Identity;
use crate::auth::jwt;
use crate::routes::SharedRuntime;

use super::{ResolvedShare, resolve_token};

/// The three possible auth situations for a public-aware handler.
pub enum CallerAuth {
    /// Valid JWT — proceed with RBAC as usual.
    User(Identity),
    /// Valid share token — call is anonymous but scoped.
    ShareToken(ResolvedShare),
    /// No Authorization header — call is only allowed if the manifest opted in.
    Anonymous,
}

impl CallerAuth {
    pub fn share_app_id(&self) -> Option<&str> {
        match self {
            Self::ShareToken(s) => Some(s.app_id.as_str()),
            _ => None,
        }
    }
}

impl FromRequestParts<SharedRuntime> for CallerAuth {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedRuntime) -> Result<Self, Self::Rejection> {
        let bearer_opt = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(|s| s.to_string());

        let Some(bearer) = bearer_opt else {
            return Ok(CallerAuth::Anonymous);
        };

        // JWT has at least two dots — cheap discriminator that doesn't leak
        // timing info between the two paths.
        if bearer.contains('.') {
            let auth_config = parts
                .extensions
                .get::<Arc<AuthConfig>>()
                .cloned()
                .ok_or_else(|| ApiError::Internal("auth not configured".into()))?;

            let claims = jwt::decode(&auth_config, &bearer)
                .map_err(|_| ApiError::Unauthorized("invalid token".into()))?;

            if claims.email.is_empty() {
                return Err(ApiError::Unauthorized("invalid token type".into()));
            }

            let user_id = claims.sub.parse()
                .map_err(|_| ApiError::Unauthorized("invalid token subject".into()))?;

            return Ok(CallerAuth::User(Identity { user_id, email: claims.email }));
        }

        // No dot → either a share token or a malformed Bearer.
        let pool = state.pool().clone();
        match resolve_token(&pool, &bearer).await {
            Some(share) => Ok(CallerAuth::ShareToken(share)),
            None => Err(ApiError::Unauthorized("invalid token".into())),
        }
    }
}

/// Load an app's manifest from the DB. Returns None if the app does not exist
/// or has no manifest stored.
pub async fn load_manifest(pool: &PgPool, app_id: &str) -> Result<Option<AppManifest>, ApiError> {
    let row: Option<(Option<JsonValue>,)> = sqlx::query_as(
        "SELECT manifest FROM rootcx_system.apps WHERE id = $1",
    )
    .bind(app_id)
    .fetch_optional(pool)
    .await?;

    let Some((Some(manifest_json),)) = row else {
        return Ok(None);
    };

    let manifest: AppManifest = serde_json::from_value(manifest_json)
        .map_err(|e| ApiError::Internal(format!("malformed manifest for app '{app_id}': {e}")))?;
    Ok(Some(manifest))
}

/// Look up the public-RPC declaration for `(app_id, method)`.
pub async fn find_public_rpc(pool: &PgPool, app_id: &str, method: &str) -> Result<Option<PublicRpc>, ApiError> {
    let manifest = load_manifest(pool, app_id).await?;
    Ok(manifest
        .and_then(|m| m.public)
        .map(|p| p.rpcs)
        .and_then(|rpcs| rpcs.into_iter().find(|r| r.name == method)))
}

/// Authorize an RPC call against the public surface.
///
/// Returns `Ok(())` if the call is allowed under the public surface.
/// Returns `Err(Unauthorized)` if the RPC is not declared public and the
/// caller is not authenticated, or `Err(Forbidden)` if the share token's
/// `context` doesn't match the request body on every key declared in `scope`.
///
/// `params` is the RPC body (params field) — what callers pass into the RPC.
pub fn authorize_public_rpc(
    decl: &PublicRpc,
    auth: &CallerAuth,
    app_id: &str,
    params: &JsonValue,
) -> Result<(), ApiError> {
    if decl.scope.is_empty() {
        // Pure anonymous public RPC — accept both anonymous AND share-token
        // sessions (a share token for THIS app — never another app).
        if let Some(share_app) = auth.share_app_id()
            && share_app != app_id {
                return Err(ApiError::Forbidden(format!(
                    "share token belongs to app '{share_app}', not '{app_id}'"
                )));
            }
        return Ok(());
    }

    // Scoped RPC — must have a share token, must match this app, must match keys.
    let CallerAuth::ShareToken(share) = auth else {
        return Err(ApiError::Unauthorized(
            "this endpoint requires a share token".into(),
        ));
    };

    if share.app_id != app_id {
        return Err(ApiError::Forbidden(format!(
            "share token belongs to app '{}', not '{app_id}'",
            share.app_id
        )));
    }

    for key in &decl.scope {
        let expected = share.context.get(key);
        let actual = params.get(key);
        match (expected, actual) {
            (Some(e), Some(a)) if e == a => continue,
            _ => {
                return Err(ApiError::Forbidden(format!(
                    "share scope mismatch on key '{key}'"
                )));
            }
        }
    }

    Ok(())
}
