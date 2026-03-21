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

impl FromRequestParts<SharedRuntime> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &SharedRuntime) -> Result<Self, Self::Rejection> {
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

        if claims.email.is_empty() {
            return Err(ApiError::Unauthorized("invalid token type".into()));
        }

        let user_id: Uuid = claims.sub.parse()
            .map_err(|_| ApiError::Unauthorized("invalid token subject".into()))?;

        Ok(Identity { user_id, email: claims.email })
    }
}
