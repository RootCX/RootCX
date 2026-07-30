use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

use super::AuthConfig;
use super::jwt;
use crate::api_error::ApiError;
use crate::routes::SharedRuntime;

#[derive(Clone, Debug)]
pub struct Identity {
    pub user_id: Uuid,
    pub email: String,
}

pub struct AudienceIdentity {
    pub identity: Identity,
    pub scopes: HashSet<String>,
}

/// Deny-by-default enablement: `false` if the principal is disabled or missing.
/// The single chokepoint that turns a decoded token (or a resolved owner) into
/// live authority, so a disabled principal loses access immediately rather than
/// at token expiry.
pub async fn principal_enabled(pool: &sqlx::PgPool, uid: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT disabled_at IS NULL FROM rootcx_system.users WHERE id = $1",
    )
    .bind(uid)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

impl Identity {
    /// Audit attribution pair `(actor, delegator)`. HTTP requests are always
    /// direct now (delegation is carried out-of-band via RpcCaller, not the
    /// JWT), so the actor is the user and there is no delegator.
    pub fn actor_pair(&self) -> (Option<Uuid>, Option<Uuid>) {
        (Some(self.user_id), None)
    }
}

fn auth_config(parts: &Parts, state: &SharedRuntime) -> Arc<AuthConfig> {
    parts
        .extensions
        .get::<Arc<AuthConfig>>()
        .cloned()
        .unwrap_or_else(|| state.auth_config().clone())
}

fn bearer_token(parts: &Parts) -> Result<&str, ApiError> {
    parts
        .headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError::Unauthorized("missing or invalid authorization header".into()))
}

/// Authenticate REST extractors and MCP middleware through the same policy.
pub async fn authenticate_parts(parts: &Parts, state: &SharedRuntime) -> Result<Identity, ApiError> {
    let claims = jwt::decode_unscoped(&auth_config(parts, state), bearer_token(parts)?)
        .map_err(|_| ApiError::Unauthorized("invalid token".into()))?;
    identity_from_claims(state, claims).await
}

pub async fn authenticate_parts_for_audience(
    parts: &Parts,
    state: &SharedRuntime,
    audience: &str,
) -> Result<AudienceIdentity, ApiError> {
    let claims = jwt::decode_for_audience(&auth_config(parts, state), bearer_token(parts)?, audience)
        .map_err(|_| ApiError::Unauthorized("invalid token audience".into()))?;
    let scopes = claims
        .scope
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let identity = identity_from_claims(state, claims).await?;
    Ok(AudienceIdentity { identity, scopes })
}

async fn identity_from_claims(state: &SharedRuntime, claims: jwt::Claims) -> Result<Identity, ApiError> {
    // Access tokens carry an email; refresh/other tokens are not accepted here.
    if claims.email.is_empty() {
        return Err(ApiError::Unauthorized("invalid token type".into()));
    }
    let user_id: Uuid = claims.sub.parse()
        .map_err(|_| ApiError::Unauthorized("invalid token subject".into()))?;
    if !principal_enabled(state.pool(), user_id).await {
        return Err(ApiError::Unauthorized("principal disabled".into()));
    }
    Ok(Identity { user_id, email: claims.email })
}

impl FromRequestParts<SharedRuntime> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedRuntime) -> Result<Self, Self::Rejection> {
        authenticate_parts(parts, state).await
    }
}
