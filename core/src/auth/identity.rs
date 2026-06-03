use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

use super::AuthConfig;
use super::jwt;
use crate::api_error::ApiError;
use crate::routes::SharedRuntime;

pub struct Identity {
    pub user_id: Uuid,
    pub email: String,
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

impl FromRequestParts<SharedRuntime> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &SharedRuntime) -> Result<Self, Self::Rejection> {
        let auth_config = parts
            .extensions
            .get::<Arc<AuthConfig>>()
            .cloned()
            .ok_or_else(|| ApiError::Internal("auth not configured".into()))?;

        let token = parts.headers.get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .ok_or_else(|| ApiError::Unauthorized("missing or invalid authorization header".into()))?;

        let claims = jwt::decode(&auth_config, token)
            .map_err(|_| ApiError::Unauthorized("invalid token".into()))?;

        // Access tokens carry an email; refresh/other tokens are not accepted here.
        if claims.email.is_empty() {
            return Err(ApiError::Unauthorized("invalid token type".into()));
        }

        let user_id: Uuid = claims.sub.parse()
            .map_err(|_| ApiError::Unauthorized("invalid token subject".into()))?;

        // A valid token is not enough: a disabled (or deleted) principal has no
        // authority, effective immediately rather than at token expiry.
        if !principal_enabled(state.pool(), user_id).await {
            return Err(ApiError::Unauthorized("principal disabled".into()));
        }

        Ok(Identity { user_id, email: claims.email })
    }
}
