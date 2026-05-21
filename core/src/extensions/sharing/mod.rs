//! Public sharing extension.
//!
//! Tokens are 32-byte CSPRNG, stored as SHA-256 hashes, compared constant-time.
//! Public access is route-level, declared by the app manifest's `public` field.
//! The share token adds a `context` payload for record-scoped access.
//! Revocation is instantaneous (lookup filters `revoked_at IS NULL`).

pub mod guard;
pub mod routes;
mod tokens;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{delete, post};
use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;

pub struct SharingExtension;

/// Resolved share record — what `resolve_token` returns to callers.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedShare {
    pub share_id: Uuid,
    pub app_id: String,
    pub context: JsonValue,
}

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

#[async_trait]
impl RuntimeExtension for SharingExtension {
    fn name(&self) -> &str { "sharing" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping sharing extension");

        exec(pool, r#"
            CREATE TABLE IF NOT EXISTS rootcx_system.public_shares (
                id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id            TEXT NOT NULL,
                token_hash        BYTEA NOT NULL,
                token_prefix      TEXT NOT NULL,
                context           JSONB NOT NULL DEFAULT '{}'::jsonb,
                created_by        UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
                revoked_at        TIMESTAMPTZ,
                last_accessed_at  TIMESTAMPTZ,
                access_count      BIGINT NOT NULL DEFAULT 0,
                password_hash     TEXT,
                expires_at        TIMESTAMPTZ
            )
        "#).await?;

        // Only one active share per (app, creator, context) — idempotent
        // toggle for the owner. We md5 the context::text so it fits in a
        // btree index regardless of payload size; collisions are not a
        // security concern here since the only effect would be an
        // unnecessary "duplicate share" rejection.
        exec(pool,
            "CREATE UNIQUE INDEX IF NOT EXISTS uq_public_shares_active \
             ON rootcx_system.public_shares (app_id, created_by, md5(context::text)) \
             WHERE revoked_at IS NULL"
        ).await?;

        // Lookup by hash (single-row by definition since the hash is unique).
        exec(pool,
            "CREATE UNIQUE INDEX IF NOT EXISTS uq_public_shares_token_hash \
             ON rootcx_system.public_shares (token_hash) \
             WHERE revoked_at IS NULL"
        ).await?;

        exec(pool,
            "CREATE INDEX IF NOT EXISTS idx_public_shares_owner \
             ON rootcx_system.public_shares (created_by)"
        ).await?;

        info!("sharing extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route(
                    "/api/v1/apps/{app_id}/public-shares",
                    post(routes::create_share).get(routes::list_shares),
                )
                .route(
                    "/api/v1/apps/{app_id}/public-shares/{share_id}",
                    delete(routes::revoke_share),
                )
                // Anonymous resolve: Bearer = share token → returns the share's
                // app_id + context. Lets a /share/:token frontend bootstrap
                // without baking the resource id into the URL.
                .route(
                    "/api/v1/public/share/info",
                    axum::routing::get(routes::share_info),
                ),
        )
    }
}

/// Lightweight lookup that only returns the app_id for a share token.
/// Used by `serve_share_frontend` to route to the correct frontend bundle
/// without bumping access counters (the real API call handles that).
pub async fn resolve_app_id(pool: &PgPool, raw: &str) -> Option<String> {
    if !tokens::is_well_formed(raw) {
        return None;
    }
    let candidate = tokens::hash(raw);
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT app_id FROM rootcx_system.public_shares \
          WHERE token_hash = $1 AND revoked_at IS NULL \
            AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(&candidate[..])
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    row.map(|(app_id,)| app_id)
}

/// Resolve a raw share token to its underlying record.
///
/// Returns None if:
/// - the token is malformed (length/charset)
/// - no row matches the hash (constant-time'd)
/// - the row is revoked or expired
///
/// The function bumps `access_count` and `last_accessed_at` as a best-effort
/// side effect; failures here are swallowed to avoid blocking the read.
pub async fn resolve_token(pool: &PgPool, raw: &str) -> Option<ResolvedShare> {
    if !tokens::is_well_formed(raw) {
        return None;
    }
    let candidate = tokens::hash(raw);

    let row: Option<(Uuid, String, JsonValue, Vec<u8>)> = sqlx::query_as(
        "SELECT id, app_id, context, token_hash \
           FROM rootcx_system.public_shares \
          WHERE token_hash = $1 AND revoked_at IS NULL \
            AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(&candidate[..])
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (share_id, app_id, context, stored) = row?;

    // Defense in depth: re-verify constant-time even though the index lookup
    // already restricted us to a single matching hash.
    if !tokens::verify(&stored, &candidate) {
        return None;
    }

    // Best-effort access bump — never block the read on this.
    let _ = sqlx::query(
        "UPDATE rootcx_system.public_shares \
            SET access_count = access_count + 1, last_accessed_at = now() \
          WHERE id = $1",
    )
    .bind(share_id)
    .execute(pool)
    .await;

    Some(ResolvedShare { share_id, app_id, context })
}
