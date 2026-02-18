use std::process::Stdio;

use axum::extract::{Multipart, Path as AxumPath, State};
use axum::Json;
use serde_json::{json, Value as JsonValue};
use tracing::info;

use crate::api_error::ApiError;
use super::{SharedRuntime, pool_and_secrets, wm};

/// POST /api/v1/apps/{app_id}/deploy — upload tar.gz, extract, install deps, start worker.
pub async fn deploy_backend(
    State(rt): State<SharedRuntime>,
    AxumPath(app_id): AxumPath<String>,
    mut multipart: Multipart,
) -> Result<Json<JsonValue>, ApiError> {
    let app_dir = rt.lock().await.data_dir().join("apps").join(&app_id);

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
        tokio::fs::remove_dir_all(&app_dir)
            .await
            .map_err(|e| ApiError::Internal(format!("clear app dir: {e}")))?;
    }
    tokio::fs::create_dir_all(&app_dir)
        .await
        .map_err(|e| ApiError::Internal(format!("create app dir: {e}")))?;

    let dir = app_dir.clone();
    tokio::task::spawn_blocking(move || {
        tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]))
            .unpack(&dir)
            .map_err(|e| ApiError::Internal(format!("extract archive: {e}")))
    })
    .await
    .map_err(|e| ApiError::Internal(format!("extract task: {e}")))?
    ?;

    info!(app_id = %app_id, dir = %app_dir.display(), "backend deployed");

    if app_dir.join("package.json").exists() {
        install_deps(&app_dir).await?;
    }

    let (pool, secrets) = pool_and_secrets(&rt).await?;
    let w = wm(&rt).await?;
    let _ = w.stop_app(&app_id).await;
    w.start_app(&pool, &secrets, &app_id).await?;

    Ok(Json(json!({ "message": format!("app '{app_id}' deployed and started") })))
}

async fn install_deps(dir: &std::path::Path) -> Result<(), ApiError> {
    let bin = if which("bun") { "bun" } else { "npm" };
    info!(bin, dir = %dir.display(), "installing dependencies");
    let out = tokio::process::Command::new(bin)
        .arg("install")
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| ApiError::Internal(format!("{bin} install: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::Internal(format!(
            "{bin} install: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

fn which(bin: &str) -> bool {
    std::process::Command::new("which")
        .arg(bin)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}
