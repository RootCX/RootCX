use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;

pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    info!("bootstrapping rootcx_system schema");

    sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system").execute(pool).await.map_err(RuntimeError::Schema)?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.apps (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            version     TEXT NOT NULL DEFAULT '0.0.1',
            status      TEXT NOT NULL DEFAULT 'installed',
            manifest    JSONB,
            icon        TEXT,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    sqlx::query("ALTER TABLE rootcx_system.apps ADD COLUMN IF NOT EXISTS icon TEXT")
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.config (
            key   TEXT PRIMARY KEY,
            value JSONB NOT NULL
        )",
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.llm_models (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            provider    TEXT NOT NULL,
            model       TEXT NOT NULL,
            config      JSONB NOT NULL DEFAULT '{}',
            is_default  BOOLEAN NOT NULL DEFAULT FALSE,
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    sqlx::query(
        "INSERT INTO rootcx_system.apps (id, name, version, status)
         VALUES ('core', 'Core', '0.0.0', 'system') ON CONFLICT (id) DO NOTHING",
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    crate::secrets::bootstrap_secrets_schema(pool).await?;
    crate::jobs::bootstrap(pool).await?;
    crate::crons::bootstrap(pool).await?;

    info!("rootcx_system schema ready");
    Ok(())
}
