use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::auth::{jwt, password, AuthConfig};
use crate::routes::SharedRuntime;

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct RefreshRequest {
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthUserResponse {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub created_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user: AuthUserResponse,
}

type UserRow = (Uuid, String, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>);

fn user_response(row: UserRow) -> AuthUserResponse {
    AuthUserResponse {
        id: row.0.to_string(),
        username: row.1,
        email: row.2,
        display_name: row.3,
        created_at: row.4.to_rfc3339(),
    }
}

/// Decode a refresh token and extract (user_id, session_id).
fn decode_refresh(config: &AuthConfig, token: &str) -> Result<(Uuid, Uuid), ApiError> {
    let claims = jwt::decode(config, token)
        .map_err(|_| ApiError::Unauthorized("invalid refresh token".into()))?;
    let session_id = claims.session_id
        .ok_or_else(|| ApiError::Unauthorized("not a refresh token".into()))?;
    let user_id: Uuid = claims.sub.parse()
        .map_err(|_| ApiError::Unauthorized("invalid token subject".into()))?;
    Ok((user_id, session_id))
}

pub async fn register(
    State(rt): State<SharedRuntime>,
    Json(req): Json<RegisterRequest>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    if req.username.is_empty() || req.password.len() < 8 {
        return Err(ApiError::BadRequest("username required, password min 8 chars".into()));
    }

    let pool = super::pool(&rt).await?;
    let pw_hash = password::hash(&req.password)?;

    let row: UserRow = sqlx::query_as(
        "INSERT INTO rootcx_system.users (username, email, display_name, password_hash)
         VALUES ($1, $2, $3, $4)
         RETURNING id, username, email, display_name, created_at",
    )
    .bind(&req.username)
    .bind(&req.email)
    .bind(&req.display_name)
    .bind(&pw_hash)
    .fetch_one(&pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            ApiError::BadRequest(format!("username '{}' already taken", req.username))
        } else {
            ApiError::Internal(e.to_string())
        }
    })?;

    Ok((StatusCode::CREATED, Json(json!({ "user": user_response(row) }))))
}

pub async fn login(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let pool = super::pool(&rt).await?;

    let row: Option<(Uuid, String, Option<String>, Option<String>, Option<String>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, username, email, display_name, password_hash, created_at
             FROM rootcx_system.users WHERE username = $1",
        )
        .bind(&req.username)
        .fetch_optional(&pool)
        .await?;

    let (user_id, username, email, display_name, pw_hash, created_at) =
        row.ok_or_else(|| ApiError::Unauthorized("invalid credentials".into()))?;

    let pw_hash = pw_hash.ok_or_else(|| ApiError::Unauthorized("password login not available".into()))?;
    if !password::verify(&req.password, &pw_hash) {
        return Err(ApiError::Unauthorized("invalid credentials".into()));
    }

    let session_id = Uuid::new_v4();
    let expires_at = chrono::Utc::now() + auth_config.refresh_ttl;
    sqlx::query("INSERT INTO rootcx_system.sessions (id, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(session_id).bind(user_id).bind(expires_at)
        .execute(&pool).await?;

    let access_token = jwt::encode_access(&auth_config, user_id, &username)?;
    let refresh_token = jwt::encode_refresh(&auth_config, user_id, session_id)?;

    Ok(Json(LoginResponse {
        access_token,
        refresh_token,
        expires_in: auth_config.access_ttl.as_secs() as i64,
        user: user_response((user_id, username, email, display_name, created_at)),
    }))
}

pub async fn refresh(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Json(req): Json<RefreshRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let (user_id, session_id) = decode_refresh(&auth_config, &req.refresh_token)?;
    let pool = super::pool(&rt).await?;

    let valid: Option<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM rootcx_system.sessions WHERE id = $1 AND expires_at > now()",
    ).bind(session_id).fetch_optional(&pool).await?;

    if valid.is_none() {
        return Err(ApiError::Unauthorized("session revoked or expired".into()));
    }

    let (username,): (String,) = sqlx::query_as(
        "SELECT username FROM rootcx_system.users WHERE id = $1",
    ).bind(user_id).fetch_optional(&pool).await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;

    let access_token = jwt::encode_access(&auth_config, user_id, &username)?;

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
    let pool = super::pool(&rt).await?;

    sqlx::query("DELETE FROM rootcx_system.sessions WHERE id = $1")
        .bind(session_id).execute(&pool).await?;

    Ok(Json(json!({ "message": "logged out" })))
}

pub async fn me(
    State(rt): State<SharedRuntime>,
    identity: Identity,
) -> Result<Json<AuthUserResponse>, ApiError> {
    let pool = super::pool(&rt).await?;

    let row: UserRow = sqlx::query_as(
        "SELECT id, username, email, display_name, created_at
         FROM rootcx_system.users WHERE id = $1",
    )
    .bind(identity.user_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound("user not found".into()))?;

    Ok(Json(user_response(row)))
}
