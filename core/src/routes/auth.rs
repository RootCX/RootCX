use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::auth::{AuthConfig, jwt};
use crate::governance::authority::require_admin;
use crate::routes::SharedRuntime;

#[derive(Deserialize)]
pub struct RefreshRequest {
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUserResponse {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: String,
}

type UserRow = (Uuid, String, Option<String>, chrono::DateTime<chrono::Utc>);

fn user_response(row: UserRow) -> AuthUserResponse {
    AuthUserResponse {
        id: row.0.to_string(),
        email: row.1,
        display_name: row.2,
        created_at: row.3.to_rfc3339(),
    }
}

/// Decode a refresh token and extract (user_id, session_id).
fn decode_refresh(config: &AuthConfig, token: &str) -> Result<(Uuid, Uuid), ApiError> {
    let claims = jwt::decode_unscoped(config, token)
        .map_err(|_| ApiError::Unauthorized("invalid refresh token".into()))?;
    let session_id = claims.session_id.ok_or_else(|| ApiError::Unauthorized("not a refresh token".into()))?;
    let user_id: Uuid = claims.sub.parse().map_err(|_| ApiError::Unauthorized("invalid token subject".into()))?;
    Ok((user_id, session_id))
}

pub async fn refresh(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let (user_id, session_id) = decode_refresh(&auth_config, &req.refresh_token)?;
    let pool = super::pool(&rt);

    let valid: Option<(Uuid,)> =
        sqlx::query_as("SELECT user_id FROM rootcx_system.sessions WHERE id = $1 AND user_id = $2 AND expires_at > now()")
            .bind(session_id)
            .bind(user_id)
            .fetch_optional(&pool)
            .await?;

    if valid.is_none() {
        return Err(ApiError::Unauthorized("session revoked or expired".into()));
    }

    let (email,): (String,) = sqlx::query_as("SELECT email FROM rootcx_system.users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&pool)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;

    let access_token = jwt::encode_access(&auth_config, user_id, &email)?;

    Ok(Json(json!({
        "accessToken": access_token,
        "expiresIn": auth_config.access_ttl.as_secs(),
    })))
}

pub async fn logout(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let (_user_id, session_id) = decode_refresh(&auth_config, &req.refresh_token)?;
    let pool = super::pool(&rt);

    sqlx::query("DELETE FROM rootcx_system.sessions WHERE id = $1").bind(session_id).execute(&pool).await?;

    Ok(Json(json!({ "message": "logged out" })))
}

pub async fn auth_mode(State(rt): State<SharedRuntime>) -> Result<Json<JsonValue>, ApiError> {
    let pool = super::pool(&rt);
    let providers: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, display_name FROM rootcx_system.oidc_providers WHERE enabled = true ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let magic_link_enabled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_permissions WHERE key = 'auth.invite')",
    )
    .fetch_one(&pool)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("magic_link_enabled check failed: {e}");
        false
    });

    Ok(Json(json!({
        "authRequired": true,
        "magicLinkEnabled": magic_link_enabled,
        "providers": providers.iter().map(|(id, name)| json!({ "id": id, "displayName": name })).collect::<Vec<_>>(),
    })))
}

pub async fn list_users(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<AuthUserResponse>>, ApiError> {
    let pool = super::pool(&rt);
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT id, email, display_name, created_at
         FROM rootcx_system.users WHERE is_system = false ORDER BY email",
    )
    .fetch_all(&pool)
    .await?;
    Ok(Json(rows.into_iter().map(user_response).collect()))
}

pub async fn me(State(rt): State<SharedRuntime>, identity: Identity) -> Result<Json<AuthUserResponse>, ApiError> {
    let pool = super::pool(&rt);

    let row: UserRow = sqlx::query_as(
        "SELECT id, email, display_name, created_at
         FROM rootcx_system.users WHERE id = $1",
    )
    .bind(identity.user_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound("user not found".into()))?;

    Ok(Json(user_response(row)))
}

pub async fn delete_user(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(target): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = super::pool(&rt);
    require_admin(&pool, identity.user_id).await?;

    // Lock admin rows to prevent concurrent revoke/delete races on the last-admin guard
    let mut tx = pool.begin().await?;
    let admin_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (SELECT 1 FROM rootcx_system.rbac_assignments WHERE role = 'admin' FOR UPDATE) t",
    ).fetch_one(&mut *tx).await?;

    let target_is_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.rbac_assignments WHERE user_id = $1 AND role = 'admin')",
    ).bind(target).fetch_one(&mut *tx).await?;

    if target_is_admin && admin_count <= 1 {
        return Err(ApiError::BadRequest("cannot delete the last admin".into()));
    }

    // ON DELETE CASCADE cleans up sessions, rbac_assignments, channel_participants, etc.
    let r = sqlx::query("DELETE FROM rootcx_system.users WHERE id = $1 AND is_system = false")
        .bind(target)
        .execute(&mut *tx)
        .await?;

    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound("user not found".into()));
    }

    tx.commit().await?;
    Ok(Json(json!({ "message": "user deleted" })))
}
