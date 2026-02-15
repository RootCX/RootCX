use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;

/// Bootstrap the `rootcx_system` schema and its seed tables.
///
/// Idempotent — safe to call on every boot.
pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    info!("bootstrapping rootcx_system schema");

    sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system")
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.apps (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            version     TEXT NOT NULL DEFAULT '0.0.1',
            status      TEXT NOT NULL DEFAULT 'installed',
            manifest    JSONB,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    // ── AI Forge tables ──────────────────────────────────────────────

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.forge_conversations (
            id          TEXT PRIMARY KEY,
            project_id  TEXT NOT NULL,
            title       TEXT,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.forge_messages (
            id               TEXT PRIMARY KEY,
            conversation_id  TEXT NOT NULL REFERENCES rootcx_system.forge_conversations(id),
            role             TEXT NOT NULL,
            content          JSONB NOT NULL,
            created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    info!("rootcx_system schema ready");
    Ok(())
}
