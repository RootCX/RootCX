use std::path::Path;
use std::process::Stdio;

use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use crate::extensions::agents;
use crate::secrets::SecretManager;
use crate::worker_manager::WorkerManager;

const APP_ID: &str = "assistant";
const RELEASE_URL: &str = "https://github.com/RootCX/ai-agent-base/releases/latest/download/backend.tar.gz";
const LOCAL_OVERRIDE_ENV: &str = "ROOTCX_ASSISTANT_DIR";

fn w(msg: impl std::fmt::Display) -> RuntimeError { RuntimeError::Worker(msg.to_string()) }

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let name = entry.file_name();
        if name == "node_modules" || name == ".git" { continue; }
        let dest_path = dst.join(&name);
        if ty.is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            copy_dir_recursive(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), dest_path)?;
        }
    }
    Ok(())
}

pub async fn seed_assistant(
    pool: &PgPool, data_dir: &Path, bun_bin: &Path,
    wm: &WorkerManager, secrets: &SecretManager,
    local_dir: Option<&Path>,
) -> Result<(), RuntimeError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.apps WHERE id = $1)",
    ).bind(APP_ID).fetch_one(pool).await.map_err(RuntimeError::Schema)?;
    if exists { return Ok(()); }

    let app_dir = data_dir.join("apps").join(APP_ID);
    std::fs::create_dir_all(&app_dir).map_err(w)?;

    let env_dir = std::env::var_os(LOCAL_OVERRIDE_ENV).map(std::path::PathBuf::from);
    if let Some(src) = local_dir.or(env_dir.as_deref()) {
        info!("seeding assistant from local override: {}", src.display());
        if !src.join("index.ts").exists() {
            return Err(w(format!("assistant directory {} does not contain index.ts", src.display())));
        }
        copy_dir_recursive(src, &app_dir).map_err(w)?;
    } else {
        info!("seeding assistant from {RELEASE_URL}");
        let bytes = reqwest::get(RELEASE_URL).await.map_err(w)?
            .error_for_status().map_err(w)?
            .bytes().await.map_err(w)?;

        let dest = app_dir.clone();
        tokio::task::spawn_blocking(move || {
            tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]))
                .unpack(&dest).map_err(w)
        }).await.map_err(w)??;
    }

    if app_dir.join("package.json").exists() {
        let out = tokio::process::Command::new(bun_bin)
            .arg("install").current_dir(&app_dir)
            .stdout(Stdio::piped()).stderr(Stdio::piped())
            .output().await.map_err(w)?;
        if !out.status.success() {
            return Err(w(format!("bun install: {}", String::from_utf8_lossy(&out.stderr))));
        }
    }

    sqlx::query(
        "INSERT INTO rootcx_system.apps (id, name, version, status, manifest)
         Values ($1, 'Assistant', '0.1.0', 'installed', $2) ON CONFLICT (id) DO NOTHING",
    ).bind(APP_ID).bind(serde_json::json!({
        "appId": APP_ID, "name": "Assistant", "version": "0.1.0", "type": "agent",
    })).execute(pool).await.map_err(RuntimeError::Schema)?;

    crate::governance::cross_app::register_installation(pool, APP_ID).await?;

    if let Some(def) = agents::config::load_agent_json(&app_dir).await {
        agents::register_agent(pool, APP_ID, &def, None).await?;
    }

    wm.start_app(pool, secrets, APP_ID).await?;
    info!("assistant agent seeded");
    Ok(())
}
