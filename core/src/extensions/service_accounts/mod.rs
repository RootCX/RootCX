//! Service accounts: non-human principals owned by the tenant. A service
//! account is just a `rootcx_system.users` row (`kind='service'`, random v4 id,
//! no password / oidc). It is governed by the same RBAC, RLS, delegation, and
//! worker isolation as every other principal. The only net-new surface is:
//! lifecycle CRUD, credential storage (reusing `auth::secure_tokens`), the
//! client-credentials token endpoint (RFC 6749 section 4.4), and act-as grants.
//! See docs/service-accounts.md.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Form, Json, Router};
use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::auth::{AuthConfig, jwt, secure_tokens};
use crate::extensions::rbac::policy::require_perm;
use crate::routes::{self, SharedRuntime};

const MANAGE: &str = "admin:service_accounts.manage";
/// The seeded internal system user; never listed or managed as a service account.
const SYSTEM_UID: Uuid = Uuid::from_u128(1);
const DEFAULT_CREDENTIAL_TTL_DAYS: i64 = 90;
/// Indexed lookup prefix stored in plaintext: `rcs_` + the first 8 token chars.
const KEY_PREFIX_LEN: usize = 12;

pub struct ServiceAccountExtension {
    pub config: Arc<AuthConfig>,
}

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

fn key_prefix(full: &str) -> String {
    full.chars().take(KEY_PREFIX_LEN).collect()
}

async fn assert_is_service_account(pool: &PgPool, id: Uuid) -> Result<(), ApiError> {
    let is_sa: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.users WHERE id = $1 AND kind = 'service')")
        .bind(id).fetch_one(pool).await?;
    if is_sa { Ok(()) } else { Err(ApiError::NotFound("service account not found".into())) }
}

#[async_trait]
impl RuntimeExtension for ServiceAccountExtension {
    fn name(&self) -> &str { "service_accounts" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.sa_credentials (
                id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                sa_user_id   UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                name         TEXT NOT NULL,
                prefix       TEXT NOT NULL,
                key_hash     BYTEA NOT NULL,
                expires_at   TIMESTAMPTZ,
                revoked_at   TIMESTAMPTZ,
                last_used_at TIMESTAMPTZ,
                created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS idx_sa_creds_prefix ON rootcx_system.sa_credentials (prefix)",
            "INSERT INTO rootcx_system.rbac_permissions (key, description) \
             VALUES ('admin:service_accounts.manage', 'Manage service accounts and their credentials') \
             ON CONFLICT (key) DO NOTHING",
            "INSERT INTO rootcx_system.rbac_permissions (key, description) \
             VALUES ('admin:rbac.escalate', 'Override the act-as anti-escalation rule') \
             ON CONFLICT (key) DO NOTHING",
        ] {
            exec(pool, ddl).await?;
        }
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/service-accounts", get(list_sa).post(create_sa))
                .route("/api/v1/service-accounts/{id}", delete(delete_sa))
                .route("/api/v1/service-accounts/{id}/disable", post(disable_sa))
                .route("/api/v1/service-accounts/{id}/enable", post(enable_sa))
                .route("/api/v1/service-accounts/{id}/credentials", post(create_credential))
                .route("/api/v1/service-accounts/{id}/credentials/{cred_id}", delete(revoke_credential))
                .route("/api/v1/service-accounts/{id}/act-as", post(grant_act_as).delete(revoke_act_as))
                .route("/api/v1/auth/token", post(issue_token))
                .layer(axum::Extension(Arc::clone(&self.config))),
        )
    }
}

// ── Lifecycle ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateSa {
    slug: String,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 48
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

async fn create_sa(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Json(body): Json<CreateSa>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = routes::pool(&rt);
    require_perm(&pool, identity.user_id, MANAGE).await?;
    if !valid_slug(&body.slug) {
        return Err(ApiError::BadRequest("slug must match [a-z0-9_-], max 48 chars".into()));
    }
    let id = Uuid::new_v4();
    let email = format!("sa+{}@localhost", body.slug);
    sqlx::query(
        "INSERT INTO rootcx_system.users (id, email, display_name, is_system, kind) \
         VALUES ($1, $2, $3, true, 'service')",
    )
    .bind(id)
    .bind(&email)
    .bind(&body.display_name)
    .execute(&pool)
    .await
    .map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            ApiError::BadRequest(format!("slug '{}' already taken", body.slug))
        } else {
            ApiError::Internal(e.to_string())
        }
    })?;
    // Born with NO permissions (deny-by-default). Admin grants a least-privilege
    // role via the standard RBAC API.
    Ok((StatusCode::CREATED, Json(json!({ "id": id, "email": email }))))
}

async fn list_sa(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    require_perm(&pool, identity.user_id, MANAGE).await?;
    let rows: Vec<(Uuid, String, Option<String>, Option<chrono::DateTime<chrono::Utc>>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, email, display_name, disabled_at, created_at \
             FROM rootcx_system.users WHERE kind = 'service' AND id <> $1 ORDER BY email",
        )
        .bind(SYSTEM_UID)
        .fetch_all(&pool)
        .await?;
    Ok(Json(json!(rows.into_iter().map(|(id, email, name, disabled, created)| json!({
        "id": id,
        "email": email,
        "displayName": name,
        "disabled": disabled.is_some(),
        "createdAt": created.to_rfc3339(),
    })).collect::<Vec<_>>())))
}

async fn disable_sa(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    set_disabled(&rt, identity.user_id, id, true).await
}

async fn enable_sa(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    set_disabled(&rt, identity.user_id, id, false).await
}

async fn set_disabled(rt: &SharedRuntime, caller: Uuid, id: Uuid, disabled: bool) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(rt);
    require_perm(&pool, caller, MANAGE).await?;
    let r = sqlx::query(
        "UPDATE rootcx_system.users SET disabled_at = CASE WHEN $2 THEN now() ELSE NULL END \
         WHERE id = $1 AND kind = 'service'",
    )
    .bind(id)
    .bind(disabled)
    .execute(&pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound("service account not found".into()));
    }
    Ok(Json(json!({ "id": id, "disabled": disabled })))
}

async fn delete_sa(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    require_perm(&pool, identity.user_id, MANAGE).await?;
    // Revoke standing delegations first (no FK on delegations), then delete the
    // user. sa_credentials cascade; cron_schedules.created_by -> NULL (denied).
    sqlx::query("UPDATE rootcx_system.delegations SET revoked_at = now() WHERE delegatee_uid = $1 AND revoked_at IS NULL")
        .bind(id).execute(&pool).await?;
    let r = sqlx::query("DELETE FROM rootcx_system.users WHERE id = $1 AND kind = 'service'")
        .bind(id).execute(&pool).await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound("service account not found".into()));
    }
    Ok(Json(json!({ "message": "service account deleted" })))
}

// ── Credentials ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateCredential {
    name: String,
    #[serde(rename = "expiresInDays")]
    expires_in_days: Option<i64>,
}

async fn create_credential(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateCredential>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = routes::pool(&rt);
    require_perm(&pool, identity.user_id, MANAGE).await?;

    assert_is_service_account(&pool, id).await?;

    // rcs_ + 43 base64url chars (256 bits). Shown once. SHA-256 at rest.
    let full = format!("rcs_{}", secure_tokens::generate());
    let prefix = key_prefix(&full);
    let hash = secure_tokens::hash(&full);
    let days = body.expires_in_days.unwrap_or(DEFAULT_CREDENTIAL_TTL_DAYS).clamp(1, 365);
    let expires_at = chrono::Utc::now() + chrono::Duration::days(days);

    let cred_id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.sa_credentials (sa_user_id, name, prefix, key_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(id)
    .bind(&body.name)
    .bind(&prefix)
    .bind(hash.as_slice())
    .bind(expires_at)
    .fetch_one(&pool)
    .await?;

    Ok((StatusCode::CREATED, Json(json!({
        "id": cred_id,
        "key": full,
        "prefix": prefix,
        "expiresAt": expires_at.to_rfc3339(),
        "note": "store this key now; it is shown only once",
    }))))
}

async fn revoke_credential(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((id, cred_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    require_perm(&pool, identity.user_id, MANAGE).await?;
    let r = sqlx::query(
        "UPDATE rootcx_system.sa_credentials SET revoked_at = now() \
         WHERE id = $1 AND sa_user_id = $2 AND revoked_at IS NULL",
    )
    .bind(cred_id).bind(id).execute(&pool).await?;
    if r.rows_affected() == 0 {
        return Err(ApiError::NotFound("credential not found".into()));
    }
    Ok(Json(json!({ "message": "credential revoked" })))
}

// ── Act-as grants ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ActAsBody {
    #[serde(rename = "userId")]
    user_id: Uuid,
}

async fn grant_act_as(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
    Json(body): Json<ActAsBody>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    require_perm(&pool, identity.user_id, MANAGE).await?;
    assert_is_service_account(&pool, id).await?;
    crate::act_as::grant(&pool, body.user_id, id).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(json!({ "message": "act-as granted" })))
}

async fn revoke_act_as(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<Uuid>,
    Json(body): Json<ActAsBody>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    require_perm(&pool, identity.user_id, MANAGE).await?;
    crate::act_as::revoke(&pool, body.user_id, id).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(json!({ "message": "act-as revoked" })))
}

// ── Client-credentials token (RFC 6749 section 4.4) ───────────────────

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: String,
    client_secret: String,
}

async fn issue_token(
    State(rt): State<SharedRuntime>,
    axum::Extension(config): axum::Extension<Arc<AuthConfig>>,
    Form(req): Form<TokenRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    if req.grant_type != "client_credentials" {
        return Err(ApiError::BadRequest("unsupported_grant_type".into()));
    }
    let pool = routes::pool(&rt);

    let sa_id: Uuid = req.client_id.parse()
        .map_err(|_| ApiError::Unauthorized("invalid_client".into()))?;

    let sa: Option<(String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT email, disabled_at FROM rootcx_system.users WHERE id = $1 AND kind = 'service'",
    ).bind(sa_id).fetch_optional(&pool).await?;
    let (email, disabled_at) = sa.ok_or_else(|| ApiError::Unauthorized("invalid_client".into()))?;
    if disabled_at.is_some() {
        return Err(ApiError::Unauthorized("invalid_client".into()));
    }

    let prefix = key_prefix(&req.client_secret);
    let candidates: Vec<(Uuid, Vec<u8>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, key_hash, expires_at FROM rootcx_system.sa_credentials \
         WHERE sa_user_id = $1 AND prefix = $2 AND revoked_at IS NULL",
    ).bind(sa_id).bind(&prefix).fetch_all(&pool).await?;

    let presented = secure_tokens::hash(&req.client_secret);
    let now = chrono::Utc::now();
    let matched = candidates.into_iter().find(|(_, stored, expires)| {
        expires.as_ref().map(|e| *e > now).unwrap_or(true) && secure_tokens::verify(stored, &presented)
    });

    let Some((cred_id, _, _)) = matched else {
        return Err(ApiError::Unauthorized("invalid_client".into()));
    };

    let _ = sqlx::query("UPDATE rootcx_system.sa_credentials SET last_used_at = now() WHERE id = $1")
        .bind(cred_id).execute(&pool).await;

    let access_token = jwt::encode_access(&config, sa_id, &email)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(json!({
        "access_token": access_token,
        "token_type": "Bearer",
        "expires_in": config.access_ttl.as_secs(),
    })))
}
