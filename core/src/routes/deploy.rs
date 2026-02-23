use std::path::Path;
use std::process::Stdio;

use axum::Json;
use axum::extract::{Multipart, Path as AxumPath, State};
use serde_json::{Value as JsonValue, json};
use tracing::info;

use super::SharedRuntime;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use rootcx_shared_types::AppManifest;

/// POST /api/v1/apps/{app_id}/deploy — upload tar.gz, extract, install deps, start worker.
pub async fn deploy_backend(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    AxumPath(app_id): AxumPath<String>,
    mut multipart: Multipart,
) -> Result<Json<JsonValue>, ApiError> {
    let (app_dir, bun_bin, pool, secrets, wm) = {
        let g = rt.lock().await;
        let app_dir = g.data_dir().join("apps").join(&app_id);
        let bun_bin = g.bun_bin().to_path_buf();
        let pool = g.pool().cloned().ok_or(ApiError::NotReady)?;
        let secrets = g.secret_manager().cloned().ok_or(ApiError::NotReady)?;
        let wm = g.worker_manager().cloned().ok_or(ApiError::NotReady)?;
        (app_dir, bun_bin, pool, secrets, wm)
    };

    let mut field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("missing 'archive' field".into()))?;

    let mut bytes = Vec::new();
    while let Some(chunk) = field.chunk().await.map_err(|e| ApiError::BadRequest(e.to_string()))? {
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(ApiError::BadRequest("empty archive".into()));
    }

    if app_dir.exists() {
        tokio::fs::remove_dir_all(&app_dir).await.map_err(|e| ApiError::Internal(format!("clear app dir: {e}")))?;
    }
    tokio::fs::create_dir_all(&app_dir).await.map_err(|e| ApiError::Internal(format!("create app dir: {e}")))?;

    let dir = app_dir.clone();
    tokio::task::spawn_blocking(move || safe_unpack(&bytes, &dir))
        .await
        .map_err(|e| ApiError::Internal(format!("extract task: {e}")))??;

    info!(app_id = %app_id, dir = %app_dir.display(), "backend deployed");

    if app_dir.join("package.json").exists() {
        install_deps(&bun_bin, &app_dir).await?;
    }

    let is_agent = app_dir.join("manifest.json").exists() && {
        let m_str = tokio::fs::read_to_string(app_dir.join("manifest.json"))
            .await
            .unwrap_or_default();
        serde_json::from_str::<AppManifest>(&m_str)
            .ok()
            .is_some_and(|m| !m.agents.is_empty())
    };

    let _ = wm.stop_app(&app_id).await;
    if is_agent {
        wm.start_agent_app(&pool, &secrets, &app_id).await?;
    } else {
        wm.start_app(&pool, &secrets, &app_id).await?;
    }

    Ok(Json(json!({ "message": format!("app '{app_id}' deployed and started") })))
}

fn safe_unpack(bytes: &[u8], dest: &std::path::Path) -> Result<(), ApiError> {
    let tar_err = |e| ApiError::Internal(format!("extract archive: {e}"));
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive.entries().map_err(tar_err)? {
        let mut entry = entry.map_err(tar_err)?;
        let path = entry.path().map_err(tar_err)?;
        if path.is_absolute() || path.components().any(|c| c == std::path::Component::ParentDir) {
            return Err(ApiError::BadRequest(format!("unsafe archive entry: {}", path.display())));
        }
        entry.unpack_in(dest).map_err(tar_err)?;
    }
    Ok(())
}

async fn install_deps(bun_bin: &Path, dir: &Path) -> Result<(), ApiError> {
    info!(bin = %bun_bin.display(), dir = %dir.display(), "installing dependencies");
    let out = tokio::process::Command::new(bun_bin)
        .arg("install")
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("{} install: {e}", bun_bin.display())))?;
    if !out.status.success() {
        return Err(ApiError::Internal(format!(
            "{} install: {}",
            bun_bin.display(),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}
