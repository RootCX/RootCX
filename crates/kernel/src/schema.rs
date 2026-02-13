use sqlx::PgPool;
use tracing::info;

use crate::KernelError;

/// Bootstrap the `rootcx_system` schema and its seed tables.
///
/// Idempotent — safe to call on every boot.
pub async fn bootstrap(pool: &PgPool) -> Result<(), KernelError> {
    info!("bootstrapping rootcx_system schema");

    sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system")
        .execute(pool)
        .await
        .map_err(KernelError::Schema)?;

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
    .map_err(KernelError::Schema)?;

    info!("rootcx_system schema ready");
    Ok(())
}
