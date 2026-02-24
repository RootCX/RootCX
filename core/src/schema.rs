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
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;

    crate::secrets::bootstrap_secrets_schema(pool).await?;
    crate::jobs::bootstrap_jobs_schema(pool).await?;

    info!("rootcx_system schema ready");
    Ok(())
}
