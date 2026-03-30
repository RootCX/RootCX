use std::path::Path;
use std::process::Stdio;

use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use crate::extensions::agents;

const APP_ID: &str = "assistant";

pub async fn seed_assistant(
    pool: &PgPool, data_dir: &Path, resources_dir: &Path, bun_bin: &Path,
) -> Result<(), RuntimeError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.apps WHERE id = $1)",
    ).bind(APP_ID).fetch_one(pool).await.map_err(RuntimeError::Schema)?;
    if exists { return Ok(()); }

    let src = resources_dir.join("assistant");
    if !src.exists() {
        tracing::warn!("assistant template not found at {}, skipping", src.display());
        return Ok(());
    }

    info!("seeding built-in assistant agent");
    let dest = data_dir.join("apps").join(APP_ID);
    copy_recursive(&src, &dest)?;

    let out = tokio::process::Command::new(bun_bin)
        .arg("install").current_dir(&dest)
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .output().await
        .map_err(|e| RuntimeError::Worker(format!("bun install: {e}")))?;
    if !out.status.success() {
        return Err(RuntimeError::Worker(format!(
            "bun install failed: {}", String::from_utf8_lossy(&out.stderr)
        )));
    }

    sqlx::query(
        "INSERT INTO rootcx_system.apps (id, name, version, status, manifest)
         VALUES ($1, 'Assistant', '0.1.0', 'installed', $2) ON CONFLICT (id) DO NOTHING",
    ).bind(APP_ID).bind(serde_json::json!({
        "appId": APP_ID, "name": "Assistant", "version": "0.1.0", "type": "agent",
    })).execute(pool).await.map_err(RuntimeError::Schema)?;

    if let Some(def) = agents::config::load_agent_json(&dest).await {
        agents::register_agent(pool, APP_ID, &def).await?;
    }
    info!("assistant agent seeded");
    Ok(())
}

fn copy_recursive(src: &Path, dest: &Path) -> Result<(), RuntimeError> {
    let e = |e: std::io::Error| RuntimeError::Worker(format!("copy: {e}"));
    std::fs::create_dir_all(dest).map_err(e)?;
    for entry in std::fs::read_dir(src).map_err(e)? {
        let entry = entry.map_err(e)?;
        let target = dest.join(entry.file_name());
        if entry.file_type().map_err(e)?.is_dir() {
            copy_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target).map_err(e)?;
        }
    }
    Ok(())
}
