//! Magic-link generate + consume.
//!
//! Generate is authenticated (Identity) and gated by either admin (`*`)
//! or the `auth.invite` permission. Privilege containment: a non-admin
//! caller can only confer roles already assigned to themselves.
//!
//! Consume is anonymous: the raw token *is* the credential. SHA-256 hash
//! lookup + atomic single-use UPDATE prevent replay and race conditions.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::auth::secure_tokens as tokens;
use crate::auth::{AuthConfig, jwt};
use crate::extensions::rbac::policy::{has_permission, resolve_permissions};
use crate::routes::{SharedRuntime, pool};

/// 15 minutes. Short enough to bound replay window if the email is leaked.
const DEFAULT_EXPIRES_IN_SECONDS: i64 = 15 * 60;
/// 24 hours. Hard cap on caller-supplied TTL.
const MAX_EXPIRES_IN_SECONDS: i64 = 24 * 60 * 60;
/// 100 roles per token is already absurd — protect against pathological input.
const MAX_ROLES_PER_TOKEN: usize = 32;

// ── /generate ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateRequest {
    pub email: String,
    #[serde(default)]
    pub roles: Vec<String>,
    pub redirect_uri: Option<String>,
    pub expires_in_seconds: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateResponse {
    pub magic_link_url: String,
    pub expires_at: DateTime<Utc>,
}

pub async fn generate(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Json(req): Json<GenerateRequest>,
) -> Result<(StatusCode, Json<GenerateResponse>), ApiError> {
    let pool = pool(&rt);

    let email = req.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::BadRequest("valid email required".into()));
    }
    if req.roles.len() > MAX_ROLES_PER_TOKEN {
        return Err(ApiError::BadRequest(format!("at most {MAX_ROLES_PER_TOKEN} roles per token")));
    }
    for role in &req.roles {
        if role.trim().is_empty() {
            return Err(ApiError::BadRequest("role names must be non-empty".into()));
        }
    }

    // Privilege check: admin OR (auth.invite AND only conferring own roles).
    let (caller_roles, caller_perms) = resolve_permissions(&pool, identity.user_id).await?;
    let is_admin = caller_perms.iter().any(|p| p == "*");
    if !is_admin {
        if !has_permission(&caller_perms, "auth.invite") {
            return Err(ApiError::Forbidden("auth.invite permission required".into()));
        }
        let owned: std::collections::HashSet<&str> = caller_roles.iter().map(|s| s.as_str()).collect();
        for role in &req.roles {
            if !owned.contains(role.as_str()) {
                return Err(ApiError::Forbidden(format!(
                    "cannot confer role '{role}' — caller does not hold it"
                )));
            }
        }
    }

    // Reject any role that doesn't exist in rbac_roles — fail-loud beats
    // emitting a magic link that grants nothing.
    if !req.roles.is_empty() {
        let existing: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM rootcx_system.rbac_roles WHERE name = ANY($1)",
        )
        .bind(&req.roles)
        .fetch_all(&pool)
        .await?;
        if existing.len() != req.roles.len() {
            let existing: std::collections::HashSet<String> = existing.into_iter().map(|(n,)| n).collect();
            let missing: Vec<&String> = req.roles.iter().filter(|r| !existing.contains(r.as_str())).collect();
            return Err(ApiError::BadRequest(format!("unknown role(s): {missing:?}")));
        }
    }

    if let Some(uri) = &req.redirect_uri
        && !is_safe_redirect_uri(uri) {
            return Err(ApiError::BadRequest("redirect_uri must be http(s) and not contain credentials".into()));
        }

    let ttl_secs = req.expires_in_seconds
        .unwrap_or(DEFAULT_EXPIRES_IN_SECONDS)
        .clamp(60, MAX_EXPIRES_IN_SECONDS);
    let expires_at = Utc::now() + chrono::Duration::seconds(ttl_secs);

    let raw = tokens::generate();
    let hash = tokens::hash(&raw);

    sqlx::query(
        "INSERT INTO rootcx_system.magic_link_tokens \
            (token_hash, email, roles, redirect_uri, created_by, expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&hash[..])
    .bind(&email)
    .bind(&req.roles)
    .bind(&req.redirect_uri)
    .bind(identity.user_id)
    .bind(expires_at)
    .execute(&pool)
    .await?;

    let magic_link_url = format!("{}/api/v1/auth/magic-link/consume?token={}", core_public_url(), raw);

    tracing::info!(
        invited_email = %email,
        roles = ?req.roles,
        created_by = %identity.user_id,
        expires_at = %expires_at,
        "magic-link generated"
    );

    Ok((StatusCode::CREATED, Json(GenerateResponse { magic_link_url, expires_at })))
}

// ── /consume ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConsumeRequest {
    pub token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user: UserPayload,
    pub redirect_uri: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserPayload {
    pub id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub created_at: String,
}

pub async fn consume(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Json(req): Json<ConsumeRequest>,
) -> Result<Json<ConsumeResponse>, ApiError> {
    let result = consume_inner(&rt, &auth_config, &req.token).await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct ConsumeQuery {
    pub token: String,
}

pub async fn consume_get(
    State(rt): State<SharedRuntime>,
    axum::Extension(auth_config): axum::Extension<Arc<AuthConfig>>,
    Query(q): Query<ConsumeQuery>,
) -> Result<Response, ApiError> {
    let result = consume_inner(&rt, &auth_config, &q.token).await?;

    if let Some(ref uri) = result.redirect_uri {
        let mut redirect_url = url::Url::parse(uri)
            .map_err(|_| ApiError::Internal("invalid stored redirect_uri".into()))?;
        let pool = pool(&rt);
        let nonce = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO rootcx_system.auth_nonces (nonce, access_token, refresh_token, expires_in)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(&nonce)
        .bind(&result.access_token)
        .bind(&result.refresh_token)
        .bind(result.expires_in)
        .execute(&pool)
        .await?;
        // DEPRECATED: also include raw tokens for SDK 0.13-0.18 backwards compat.
        // New SDK (0.19+) uses auth_nonce; legacy SDKs read query or fragment.
        let token_params = format!(
            "access_token={}&refresh_token={}&expires_in={}",
            result.access_token, result.refresh_token, result.expires_in
        );
        redirect_url.query_pairs_mut()
            .append_pair("auth_nonce", &nonce)
            .append_pair("access_token", &result.access_token)
            .append_pair("refresh_token", &result.refresh_token)
            .append_pair("expires_in", &result.expires_in.to_string());
        redirect_url.set_fragment(Some(&token_params));
        Ok((
            [(header::REFERRER_POLICY, "no-referrer")],
            Redirect::temporary(redirect_url.as_str()),
        ).into_response())
    } else {
        Ok(Json(result).into_response())
    }
}

async fn consume_inner(
    rt: &SharedRuntime,
    auth_config: &AuthConfig,
    raw_token: &str,
) -> Result<ConsumeResponse, ApiError> {
    if !tokens::is_well_formed(raw_token) {
        return Err(ApiError::Unauthorized("invalid token".into()));
    }
    let candidate = tokens::hash(raw_token);

    let pool = pool(rt);
    let mut tx = pool.begin().await?;

    let row: Option<(String, Vec<String>, Option<String>)> = sqlx::query_as(
        "UPDATE rootcx_system.magic_link_tokens \
            SET consumed_at = now() \
          WHERE token_hash = $1 \
            AND consumed_at IS NULL \
            AND expires_at > now() \
        RETURNING email, roles, redirect_uri",
    )
    .bind(&candidate[..])
    .fetch_optional(&mut *tx)
    .await?;

    let (email, roles, redirect_uri) = row.ok_or_else(|| {
        tracing::warn!("magic-link consume failed: token invalid, consumed, or expired");
        ApiError::Unauthorized("token invalid, already used, or expired".into())
    })?;

    let user_row: Option<(Uuid, String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, email, display_name, created_at FROM rootcx_system.users WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(&mut *tx)
    .await?;

    let (user_id, email, display_name, created_at) = match user_row {
        Some(u) => u,
        None => sqlx::query_as(
            "INSERT INTO rootcx_system.users (email, display_name) VALUES ($1, NULL) \
             RETURNING id, email, display_name, created_at",
        )
        .bind(&email)
        .fetch_one(&mut *tx)
        .await?,
    };

    if !roles.is_empty() {
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_assignments (user_id, role) \
             SELECT $1, unnest($2::text[]) ON CONFLICT DO NOTHING",
        )
        .bind(user_id)
        .bind(&roles)
        .execute(&mut *tx)
        .await?;
    }

    let session_id = Uuid::new_v4();
    let session_expires = Utc::now() + auth_config.refresh_ttl;
    sqlx::query(
        "INSERT INTO rootcx_system.sessions (id, user_id, expires_at) VALUES ($1, $2, $3)",
    )
    .bind(session_id)
    .bind(user_id)
    .bind(session_expires)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let access_token = jwt::encode_access(auth_config, user_id, &email)?;
    let refresh_token = jwt::encode_refresh(auth_config, user_id, session_id)?;

    // Opportunistic prune (~0.4% of calls). Fire-and-forget via spawn to
    // avoid blocking the response with an unindexed DELETE.
    if rand::random::<u8>() == 0 {
        let pool = pool.clone();
        tokio::spawn(async move {
            if let Err(e) = sqlx::query(
                "DELETE FROM rootcx_system.magic_link_tokens WHERE expires_at < now() - interval '7 days'",
            )
            .execute(&pool)
            .await
            {
                tracing::warn!("magic-link cleanup failed: {e}");
            }
        });
    }

    tracing::info!(user_id = %user_id, email = %email, roles_conferred = ?roles, "magic-link consumed");

    Ok(ConsumeResponse {
        access_token,
        refresh_token,
        expires_in: auth_config.access_ttl.as_secs() as i64,
        user: UserPayload {
            id: user_id.to_string(),
            email,
            display_name,
            created_at: created_at.to_rfc3339(),
        },
        redirect_uri,
    })
}

// ── helpers ────────────────────────────────────────────────────────────────

fn core_public_url() -> String {
    std::env::var("ROOTCX_PUBLIC_URL")
        .or_else(|_| std::env::var("ROOTCX_URL"))
        .unwrap_or_else(|_| "http://localhost:9100".to_string())
}

/// Conservative redirect_uri validator.
/// - Must be http/https
/// - Must not embed userinfo (`user:pass@`) — common phishing vector
/// - No further whitelisting at this stage: callers are expected to be
///   trusted apps (the generate endpoint is itself behind auth.invite).
fn is_safe_redirect_uri(raw: &str) -> bool {
    let Ok(url) = url::Url::parse(raw) else { return false };
    matches!(url.scheme(), "http" | "https") && url.username().is_empty() && url.password().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_redirect_accepts_http_and_https() {
        assert!(is_safe_redirect_uri("https://pulsecrm.foundation/auth/callback"));
        assert!(is_safe_redirect_uri("http://localhost:3000/callback"));
    }

    #[test]
    fn safe_redirect_rejects_userinfo() {
        assert!(!is_safe_redirect_uri("https://attacker:pwd@victim.com/"));
        assert!(!is_safe_redirect_uri("https://attacker@victim.com/"));
    }

    #[test]
    fn safe_redirect_rejects_non_http_schemes() {
        assert!(!is_safe_redirect_uri("javascript:alert(1)"));
        assert!(!is_safe_redirect_uri("file:///etc/passwd"));
        assert!(!is_safe_redirect_uri("data:text/html,<script>"));
    }

    #[test]
    fn safe_redirect_rejects_malformed() {
        assert!(!is_safe_redirect_uri("not a url"));
        assert!(!is_safe_redirect_uri(""));
    }
}
