//! Magic-link authentication extension.
//!
//! Two endpoints, no email delivery here:
//!   - POST /api/v1/auth/magic-link/generate — auth required (auth.invite perm)
//!     Returns { magicLinkUrl, expiresAt } for the caller to deliver.
//!   - POST /api/v1/auth/magic-link/consume — anonymous
//!     Single-use atomic exchange → access+refresh JWT.
//!
//! Tokens: 256-bit CSPRNG, SHA-256 stored, constant-time verify
//! (reuses crate::auth::secure_tokens). Permission `auth.invite` is
//! seeded as a core-level RBAC permission.

mod routes;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;

pub struct MagicLinkExtension;

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

#[async_trait]
impl RuntimeExtension for MagicLinkExtension {
    fn name(&self) -> &str { "magic_link" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping magic_link extension");

        exec(pool,
            "CREATE TABLE IF NOT EXISTS rootcx_system.magic_link_tokens (
                token_hash     BYTEA PRIMARY KEY,
                email          TEXT NOT NULL,
                roles          TEXT[] NOT NULL DEFAULT '{}',
                redirect_uri   TEXT,
                token_delivery TEXT NOT NULL DEFAULT 'query',
                created_by     UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
                consumed_at    TIMESTAMPTZ,
                expires_at     TIMESTAMPTZ NOT NULL
            )",
        ).await?;

        // Existing deployments: backfill the column (defaults to legacy 'query').
        exec(pool,
            "ALTER TABLE rootcx_system.magic_link_tokens \
             ADD COLUMN IF NOT EXISTS token_delivery TEXT NOT NULL DEFAULT 'query'",
        ).await?;

        // Shared nonce store (also bootstrapped by oidc; idempotent).
        crate::auth::token_delivery::ensure_schema(pool).await?;

        exec(pool,
            "CREATE INDEX IF NOT EXISTS idx_magic_link_tokens_unconsumed \
             ON rootcx_system.magic_link_tokens (expires_at) \
             WHERE consumed_at IS NULL",
        ).await?;

        // Register the core-level invite permission so admins can grant it
        // to any role (e.g. give "manager" the right to invite users).
        exec(pool,
            "INSERT INTO rootcx_system.rbac_permissions (key, description, source_app) \
             VALUES ('auth.invite', 'Generate magic-link invitations', NULL) \
             ON CONFLICT (key) DO NOTHING",
        ).await?;

        let pruned = sqlx::query(
            "DELETE FROM rootcx_system.magic_link_tokens \
              WHERE expires_at < now() - interval '7 days'",
        )
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
        if pruned.rows_affected() > 0 {
            info!(count = pruned.rows_affected(), "pruned expired magic-link tokens");
        }

        info!("magic_link extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new()
            .route("/api/v1/auth/magic-link/generate", post(routes::generate))
            .route("/api/v1/auth/magic-link/consume", get(routes::consume_get).post(routes::consume)))
    }
}
