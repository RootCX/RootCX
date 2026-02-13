use std::path::Path;

use sqlx::PgPool;
use tracing::{info, warn};

use crate::KernelError;
use crate::manifest;
use rootcx_shared_types::AppManifest;

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

/// Bootstrap system schema, then install all bundled apps from manifests.
///
/// `apps_dir` should point to the `resources/apps/` directory containing
/// subdirectories with manifest.json files.
pub async fn bootstrap_with_apps(pool: &PgPool, apps_dir: &Path) -> Result<(), KernelError> {
    bootstrap(pool).await?;

    if !apps_dir.exists() {
        warn!(path = %apps_dir.display(), "apps directory not found, skipping app installation");
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(apps_dir)
        .map_err(|e| KernelError::Schema(sqlx::Error::Io(e)))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    // Sort for deterministic install order
    entries.sort_by_key(|e| e.file_name());

    let mut installed = 0;
    for entry in &entries {
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| KernelError::Schema(sqlx::Error::Io(e)))?;

        let app_manifest: AppManifest = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                warn!(
                    path = %manifest_path.display(),
                    error = %e,
                    "failed to parse manifest, skipping"
                );
                continue;
            }
        };

        manifest::install_app(pool, &app_manifest).await?;
        installed += 1;
    }

    info!(count = installed, "all bundled apps installed");
    Ok(())
}
