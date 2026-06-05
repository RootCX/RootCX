//! Post-authentication token delivery for browser redirect flows.
//!
//! Single source of truth shared by the OIDC callback and the magic-link
//! consume redirect. Aligned with industry best practice (Supabase/Hydra/Auth0):
//! tokens are NEVER pre-generated and stored; only a hashed nonce + session
//! metadata lives in the DB. Tokens are minted fresh at exchange time.

use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use sqlx::PgPool;
use uuid::Uuid;

use super::{AuthConfig, jwt, secure_tokens as tokens};
use crate::RuntimeError;
use crate::api_error::ApiError;

/// How issued tokens reach the browser after a redirect-based login.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Delivery {
    /// Secure: a hashed single-use nonce references session metadata; tokens
    /// are minted at exchange time and never touch the DB.
    Nonce,
    /// DEPRECATED: raw tokens in the URL query + fragment. Retained for SDKs
    /// predating nonce exchange (< 0.19). Never selected for new clients.
    #[default]
    Legacy,
}

impl Delivery {
    pub fn from_param(s: &str) -> Self {
        if s == "nonce" { Self::Nonce } else { Self::Legacy }
    }
}

/// Idempotently create the nonce store. No tokens are stored -- only hashed
/// nonce + session reference (user_id, session_id for JWT minting at exchange).
pub async fn ensure_schema(pool: &PgPool) -> Result<(), RuntimeError> {
    for ddl in [
        "CREATE TABLE IF NOT EXISTS rootcx_system.auth_nonces (
            nonce_hash   BYTEA PRIMARY KEY,
            user_id      UUID NOT NULL,
            session_id   UUID NOT NULL,
            created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
        "CREATE INDEX IF NOT EXISTS idx_auth_nonces_created ON rootcx_system.auth_nonces (created_at)",
    ] {
        sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
    }
    Ok(())
}

const NONCE_TTL_SECS: f64 = 30.0;

/// Consume a single-use nonce: hash the raw nonce, atomically delete the
/// matching row (single-use + TTL), then mint fresh tokens from the stored
/// session metadata. Returns `(access_token, refresh_token, expires_in)`.
pub async fn exchange(
    pool: &PgPool,
    auth_config: &AuthConfig,
    raw_nonce: &str,
) -> Result<(String, String, i64), ApiError> {
    let nonce_hash = tokens::hash(raw_nonce);
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "DELETE FROM rootcx_system.auth_nonces n
         USING rootcx_system.users u
         WHERE u.id = n.user_id
           AND n.nonce_hash = $1
           AND n.created_at > now() - make_interval(secs => $2)
         RETURNING n.user_id, n.session_id, u.email",
    )
    .bind(nonce_hash.as_slice())
    .bind(NONCE_TTL_SECS)
    .fetch_optional(pool)
    .await?;

    let (user_id, session_id, email) = row
        .ok_or_else(|| ApiError::Unauthorized("invalid or expired nonce".into()))?;

    let access_token = jwt::encode_access(auth_config, user_id, &email)?;
    let refresh_token = jwt::encode_refresh(auth_config, user_id, session_id)?;

    Ok((access_token, refresh_token, auth_config.access_ttl.as_secs() as i64))
}

/// Drop nonces past their TTL. Boot-time hygiene.
pub async fn prune_expired(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query("DELETE FROM rootcx_system.auth_nonces WHERE created_at < now() - make_interval(secs => $1)")
        .bind(NONCE_TTL_SECS)
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
    Ok(())
}

/// Build the post-login redirect. In nonce mode, stores only a hashed nonce +
/// session metadata (never tokens). In legacy mode, mints and appends tokens.
pub async fn deliver(
    pool: &PgPool,
    mut redirect_url: url::Url,
    user_id: Uuid,
    session_id: Uuid,
    auth_config: &AuthConfig,
    email: &str,
    mode: Delivery,
) -> Result<Response, ApiError> {
    match mode {
        Delivery::Nonce => {
            let raw_nonce = tokens::generate();
            let nonce_hash = tokens::hash(&raw_nonce);
            sqlx::query(
                "INSERT INTO rootcx_system.auth_nonces (nonce_hash, user_id, session_id)
                 VALUES ($1, $2, $3)",
            )
            .bind(nonce_hash.as_slice())
            .bind(user_id)
            .bind(session_id)
            .execute(pool)
            .await?;
            redirect_url.query_pairs_mut().append_pair("auth_nonce", &raw_nonce);
        }
        Delivery::Legacy => {
            let access_token = jwt::encode_access(auth_config, user_id, email)?;
            let refresh_token = jwt::encode_refresh(auth_config, user_id, session_id)?;
            let expires_in = auth_config.access_ttl.as_secs();
            let fragment = format!(
                "access_token={access_token}&refresh_token={refresh_token}&expires_in={expires_in}"
            );
            redirect_url
                .query_pairs_mut()
                .append_pair("access_token", &access_token)
                .append_pair("refresh_token", &refresh_token)
                .append_pair("expires_in", &expires_in.to_string());
            redirect_url.set_fragment(Some(&fragment));
        }
    }
    Ok((
        [(header::REFERRER_POLICY, "no-referrer")],
        Redirect::temporary(redirect_url.as_str()),
    )
        .into_response())
}
