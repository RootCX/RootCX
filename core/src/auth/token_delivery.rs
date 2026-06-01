//! Post-authentication token delivery for browser redirect flows.
//!
//! Single source of truth shared by the OIDC callback and the magic-link
//! consume redirect. Both flows face the same security-sensitive choice
//! (whether issued tokens touch the URL), and previously each implemented it
//! independently — which let magic-link silently regress into leaking tokens
//! while OIDC was gated. Routing both through `deliver` makes divergence
//! structurally impossible.

use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;
use crate::api_error::ApiError;

/// How issued tokens reach the browser after a redirect-based login.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Delivery {
    /// Secure: tokens are stored server-side under a single-use nonce; only the
    /// nonce travels in the URL. The SDK (>= 0.19) exchanges it via POST.
    Nonce,
    /// DEPRECATED: raw tokens in the URL query + fragment. Retained for SDKs
    /// predating nonce exchange (< 0.19). Leaks tokens to access logs, browser
    /// history, and `Referer`. Never selected for new clients.
    #[default]
    Legacy,
}

impl Delivery {
    /// Parse the client-supplied `token_delivery` preference. Only the explicit
    /// opt-in `"nonce"` is secure; anything else (including the absence of the
    /// param, stored as `"query"`) stays legacy for backwards compatibility.
    pub fn from_param(s: &str) -> Self {
        if s == "nonce" { Self::Nonce } else { Self::Legacy }
    }
}

/// Idempotently create the nonce store. Owned here (not by a single extension)
/// because both OIDC and magic-link depend on it; callers invoke this from
/// their own bootstrap so neither is coupled to the other's lifecycle.
pub async fn ensure_schema(pool: &PgPool) -> Result<(), RuntimeError> {
    for ddl in [
        "CREATE TABLE IF NOT EXISTS rootcx_system.auth_nonces (
            nonce           TEXT PRIMARY KEY,
            access_token    TEXT NOT NULL,
            refresh_token   TEXT NOT NULL,
            expires_in      BIGINT NOT NULL,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
        "CREATE INDEX IF NOT EXISTS idx_auth_nonces_created ON rootcx_system.auth_nonces (created_at)",
    ] {
        sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
    }
    Ok(())
}

/// Nonce delivery window: long enough for the browser redirect round-trip,
/// short enough to bound how long plaintext tokens sit in `auth_nonces`.
const NONCE_TTL_SECS: f64 = 30.0;

/// Consume a single-use delivery nonce. The atomic `DELETE … RETURNING`
/// enforces single-use; the TTL window rejects stale nonces. Returns
/// `(access_token, refresh_token, expires_in)`.
pub async fn exchange(pool: &PgPool, nonce: &str) -> Result<(String, String, i64), ApiError> {
    let row: Option<(String, String, i64)> = sqlx::query_as(
        "DELETE FROM rootcx_system.auth_nonces
         WHERE nonce = $1 AND created_at > now() - make_interval(secs => $2)
         RETURNING access_token, refresh_token, expires_in",
    )
    .bind(nonce)
    .bind(NONCE_TTL_SECS)
    .fetch_optional(pool)
    .await?;
    row.ok_or_else(|| ApiError::Unauthorized("invalid or expired nonce".into()))
}

/// Drop nonces past their TTL. Best-effort boot-time hygiene; runtime
/// correctness already comes from the TTL guard in `exchange`.
pub async fn prune_expired(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query(
        "DELETE FROM rootcx_system.auth_nonces WHERE created_at < now() - make_interval(secs => $1)",
    )
    .bind(NONCE_TTL_SECS)
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;
    Ok(())
}

/// Build the post-login redirect, delivering the tokens per `mode`. Always
/// pins `Referrer-Policy: no-referrer` regardless of mode.
pub async fn deliver(
    pool: &PgPool,
    mut redirect_url: url::Url,
    access_token: &str,
    refresh_token: &str,
    expires_in: i64,
    mode: Delivery,
) -> Result<Response, ApiError> {
    match mode {
        Delivery::Nonce => {
            let nonce = Uuid::new_v4().to_string();
            sqlx::query(
                "INSERT INTO rootcx_system.auth_nonces (nonce, access_token, refresh_token, expires_in)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&nonce)
            .bind(access_token)
            .bind(refresh_token)
            .bind(expires_in)
            .execute(pool)
            .await?;
            redirect_url.query_pairs_mut().append_pair("auth_nonce", &nonce);
        }
        Delivery::Legacy => {
            // Tokens in query (SDK 0.13-0.16) AND fragment (SDK 0.17-0.18).
            let fragment = format!(
                "access_token={access_token}&refresh_token={refresh_token}&expires_in={expires_in}"
            );
            redirect_url
                .query_pairs_mut()
                .append_pair("access_token", access_token)
                .append_pair("refresh_token", refresh_token)
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
