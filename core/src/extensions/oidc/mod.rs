mod routes;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post, delete};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;
use crate::secrets::SecretManager;

pub struct OidcExtension;

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

#[async_trait]
impl RuntimeExtension for OidcExtension {
    fn name(&self) -> &str { "oidc" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping OIDC extension");

        for ddl in [
            // OIDC identity providers
            "CREATE TABLE IF NOT EXISTS rootcx_system.oidc_providers (
                id              TEXT PRIMARY KEY,
                display_name    TEXT NOT NULL,
                issuer_url      TEXT NOT NULL,
                client_id       TEXT NOT NULL,
                client_secret   TEXT,
                scopes          TEXT[] NOT NULL DEFAULT '{openid,email,profile}',
                auto_register   BOOLEAN NOT NULL DEFAULT true,
                default_role    TEXT NOT NULL DEFAULT 'admin',
                role_claim      TEXT,
                enabled         BOOLEAN NOT NULL DEFAULT true,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            // Temporary OIDC authorization state (browser flow)
            "CREATE TABLE IF NOT EXISTS rootcx_system.oidc_state (
                state           TEXT PRIMARY KEY,
                provider_id     TEXT NOT NULL,
                nonce           TEXT NOT NULL,
                pkce_verifier   TEXT NOT NULL,
                redirect_uri    TEXT NOT NULL,
                created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS idx_oidc_state_created
                ON rootcx_system.oidc_state (created_at)",
            // Add OIDC columns to users (idempotent)
            "ALTER TABLE rootcx_system.users ADD COLUMN IF NOT EXISTS oidc_provider TEXT",
            "ALTER TABLE rootcx_system.users ADD COLUMN IF NOT EXISTS oidc_sub TEXT",
        ] {
            exec(pool, ddl).await?;
        }

        // Unique index on (oidc_provider, oidc_sub) where not null
        // Use DO block for idempotency since CREATE UNIQUE INDEX IF NOT EXISTS
        // doesn't support WHERE clause in older postgres
        exec(pool,
            "DO $$ BEGIN
                IF NOT EXISTS (
                    SELECT 1 FROM pg_indexes
                    WHERE indexname = 'idx_users_oidc'
                ) THEN
                    CREATE UNIQUE INDEX idx_users_oidc
                        ON rootcx_system.users (oidc_provider, oidc_sub)
                        WHERE oidc_provider IS NOT NULL;
                END IF;
            END $$"
        ).await?;

        // Prune expired states
        let pruned = sqlx::query(
            "DELETE FROM rootcx_system.oidc_state WHERE created_at < now() - interval '10 minutes'",
        )
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

        if pruned.rows_affected() > 0 {
            info!(count = pruned.rows_affected(), "pruned expired OIDC states");
        }

        info!("OIDC extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new()
            .route("/api/v1/auth/oidc/providers", get(routes::list_providers).post(routes::upsert_provider))
            .route("/api/v1/auth/oidc/providers/{id}", delete(routes::delete_provider))
            .route("/api/v1/auth/oidc/token-exchange", post(routes::token_exchange))
            .route("/api/v1/auth/oidc/{provider_id}/authorize", get(routes::authorize))
            .route("/api/v1/auth/oidc/callback", get(routes::callback)))
    }
}

/// Seed an OIDC provider from environment variables (cloud provisioning).
/// Called once at boot, after extensions bootstrap and SecretManager init.
pub async fn seed_from_env(pool: &PgPool, secrets: &SecretManager) -> Result<(), RuntimeError> {
    let issuer = match std::env::var("ROOTCX_OIDC_ISSUER") {
        Ok(v) if !v.is_empty() => v,
        _ => return Ok(()), // No env vars → skip (self-hosted without OIDC, or already configured)
    };
    let client_id = std::env::var("ROOTCX_OIDC_CLIENT_ID")
        .map_err(|_| RuntimeError::Config("ROOTCX_OIDC_ISSUER set but ROOTCX_OIDC_CLIENT_ID missing".into()))?;
    let client_secret = std::env::var("ROOTCX_OIDC_CLIENT_SECRET")
        .map_err(|_| RuntimeError::Config("ROOTCX_OIDC_ISSUER set but ROOTCX_OIDC_CLIENT_SECRET missing".into()))?;

    // Check if provider already exists
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.oidc_providers WHERE id = 'rootcx')",
    )
    .fetch_one(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    if exists {
        return Ok(());
    }

    // Insert provider
    sqlx::query(
        "INSERT INTO rootcx_system.oidc_providers
            (id, display_name, issuer_url, client_id, client_secret, auto_register, default_role)
         VALUES ('rootcx', 'RootCX', $1, $2, NULL, true, 'admin')",
    )
    .bind(&issuer)
    .bind(&client_id)
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    // Encrypt client_secret in vault
    secrets.set(pool, "oidc:rootcx", "client_secret", &client_secret).await?;

    info!(issuer = %issuer, client_id = %client_id, "OIDC provider 'rootcx' seeded from env vars");
    Ok(())
}
