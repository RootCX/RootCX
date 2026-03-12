use std::path::{Path, PathBuf};
use std::process::Stdio;

use axum::Json;
use axum::extract::{Multipart, Path as AxumPath, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use serde_json::{Value as JsonValue, json};
use tracing::info;

use super::SharedRuntime;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;

async fn read_archive(multipart: &mut Multipart) -> Result<Vec<u8>, ApiError> {
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
    Ok(bytes)
}

async fn extract_to(bytes: Vec<u8>, dest: &Path) -> Result<(), ApiError> {
    if dest.exists() {
        tokio::fs::remove_dir_all(dest).await.map_err(|e| ApiError::Internal(format!("clear dir: {e}")))?;
    }
    tokio::fs::create_dir_all(dest).await.map_err(|e| ApiError::Internal(format!("create dir: {e}")))?;
    let d = dest.to_path_buf();
    tokio::task::spawn_blocking(move || safe_unpack(&bytes, &d))
        .await
        .map_err(|e| ApiError::Internal(format!("extract task: {e}")))?
}

pub async fn deploy_backend(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    AxumPath(app_id): AxumPath<String>,
    mut multipart: Multipart,
) -> Result<Json<JsonValue>, ApiError> {
    if app_id == "core" {
        return Err(ApiError::BadRequest("reserved app_id".into()));
    }

    let (app_dir, bun_bin, pool, secrets, wm) = {
        let g = rt.lock().await;
        let app_dir = g.data_dir().join("apps").join(&app_id);
        let bun_bin = g.bun_bin().to_path_buf();
        let pool = g.pool().cloned().ok_or(ApiError::NotReady)?;
        let secrets = g.secret_manager().cloned().ok_or(ApiError::NotReady)?;
        let wm = g.worker_manager().cloned().ok_or(ApiError::NotReady)?;
        (app_dir, bun_bin, pool, secrets, wm)
    };

    let bytes = read_archive(&mut multipart).await?;
    extract_to(bytes, &app_dir).await?;

    info!(app_id = %app_id, dir = %app_dir.display(), "backend deployed");

    if app_dir.join("package.json").exists() {
        install_deps(&bun_bin, &app_dir).await?;
    }

    if let Some(def) = crate::extensions::agents::config::load_agent_json(&app_dir).await {
        crate::extensions::agents::register_agent(&pool, &app_id, &def)
            .await
            .map_err(|e| ApiError::Internal(format!("agent registration: {e}")))?;
    }

    let _ = wm.stop_app(&app_id).await;
    wm.start_app(&pool, &secrets, &app_id).await?;

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

// ── Frontend deploy & serve ──────────────────────────────────────────

pub async fn deploy_frontend(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    AxumPath(app_id): AxumPath<String>,
    mut multipart: Multipart,
) -> Result<Json<JsonValue>, ApiError> {
    if app_id == "core" {
        return Err(ApiError::BadRequest("reserved app_id".into()));
    }

    let frontend_dir = rt.lock().await.data_dir().join("frontends").join(&app_id);
    let bytes = read_archive(&mut multipart).await?;
    extract_to(bytes, &frontend_dir).await?;

    info!(app_id = %app_id, dir = %frontend_dir.display(), "frontend deployed");
    Ok(Json(json!({ "message": format!("frontend for '{app_id}' deployed"), "url": format!("/apps/{app_id}/") })))
}

pub fn list_frontends(data_dir: &Path) -> std::collections::HashSet<String> {
    let dir = data_dir.join("frontends");
    std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("index.html").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

pub async fn serve_frontend(
    State(rt): State<SharedRuntime>,
    AxumPath((app_id, path)): AxumPath<(String, String)>,
) -> impl IntoResponse {
    let frontend_dir = rt.lock().await.data_dir().join("frontends").join(&app_id);

    let requested = PathBuf::from(&path);
    if requested.components().any(|c| c == std::path::Component::ParentDir) {
        return not_found();
    }

    let file_path = frontend_dir.join(&requested);
    let is_asset = file_path.is_file();
    let file_path = if is_asset { file_path } else { frontend_dir.join("index.html") };

    match tokio::fs::read(&file_path).await {
        Ok(bytes) => {
            let ct = content_type(&file_path).to_string();
            // Hashed asset filenames are immutable; HTML needs revalidation
            let cache = if is_asset { "public, max-age=31536000, immutable" } else { "public, max-age=60, must-revalidate" };
            (StatusCode::OK, [(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, cache.to_string())], bytes)
        }
        Err(_) => not_found(),
    }
}

fn not_found() -> (StatusCode, [(header::HeaderName, String); 2], Vec<u8>) {
    (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "text/plain".to_string()), (header::CACHE_CONTROL, "no-cache".to_string())], b"not found".to_vec())
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("ico") => "image/x-icon",
        Some("wasm") => "application/wasm",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    }
}
