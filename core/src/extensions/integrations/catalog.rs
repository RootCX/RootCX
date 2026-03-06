use std::path::PathBuf;
use std::process::Stdio;

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value as JsonValue, json};
use tracing::info;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;

async fn catalog_dir(rt: &SharedRuntime) -> Result<PathBuf, ApiError> {
    Ok(rt.lock().await.resources_dir().join("integrations"))
}

pub async fn list_catalog(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let dir = catalog_dir(&rt).await?;
    if !dir.exists() {
        return Ok(Json(vec![]));
    }

    let mut entries = Vec::new();
    let mut rd = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| ApiError::Internal(format!("read catalog dir: {e}")))?;

    while let Some(entry) = rd.next_entry().await.map_err(|e| ApiError::Internal(e.to_string()))? {
        let manifest_path = entry.path().join("manifest.json");
        if !manifest_path.exists() { continue; }

        let raw = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| ApiError::Internal(format!("read manifest: {e}")))?;
        let manifest: JsonValue = serde_json::from_str(&raw)
            .map_err(|e| ApiError::Internal(format!("parse manifest: {e}")))?;

        let dir_name = entry.file_name().to_string_lossy().to_string();
        let id = manifest.get("appId").and_then(|v| v.as_str()).unwrap_or(&dir_name);

        entries.push(json!({
            "id": id,
            "name": manifest.get("name").and_then(|v| v.as_str()).unwrap_or(id),
            "version": manifest.get("version").and_then(|v| v.as_str()).unwrap_or("0.0.0"),
            "description": manifest.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "actions": manifest.get("actions").unwrap_or(&json!([])),
            "configSchema": manifest.get("configSchema"),
            "userAuth": manifest.get("userAuth"),
            "webhooks": manifest.get("webhooks").unwrap_or(&json!([])),
            "instructions": manifest.get("instructions"),
        }));
    }

    Ok(Json(entries))
}

pub async fn deploy_from_catalog(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let (catalog_path, app_dir, bun_bin, pool, secrets, wm, tools) = {
        let g = rt.lock().await;
        (
            g.resources_dir().join("integrations").join(&id),
            g.data_dir().join("apps").join(&id),
            g.bun_bin().to_path_buf(),
            g.pool().cloned().ok_or(ApiError::NotReady)?,
            g.secret_manager().cloned().ok_or(ApiError::NotReady)?,
            g.worker_manager().cloned().ok_or(ApiError::NotReady)?,
            g.tool_registry().all_summaries(),
        )
    };

    if !catalog_path.exists() {
        return Err(ApiError::NotFound(format!("integration '{id}' not in catalog")));
    }

    let manifest_raw = tokio::fs::read_to_string(catalog_path.join("manifest.json"))
        .await
        .map_err(|e| ApiError::Internal(format!("read manifest: {e}")))?;
    let manifest: rootcx_types::AppManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| ApiError::Internal(format!("parse manifest: {e}")))?;

    // Brief re-lock: install_app needs &[Box<dyn RuntimeExtension>] which borrows Runtime
    {
        let g = rt.lock().await;
        crate::manifest::install_app(&pool, &manifest, g.extensions(), uuid::Uuid::nil(), &tools).await?;
    }

    if app_dir.exists() {
        tokio::fs::remove_dir_all(&app_dir).await.map_err(|e| ApiError::Internal(format!("clear: {e}")))?;
    }
    copy_dir_recursive(&catalog_path, &app_dir).await?;
    info!(id = %id, "integration copied to apps dir");

    if app_dir.join("package.json").exists() {
        let out = tokio::process::Command::new(&bun_bin)
            .arg("install")
            .current_dir(&app_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ApiError::Internal(format!("bun install: {e}")))?;
        if !out.status.success() {
            return Err(ApiError::Internal(format!("bun install: {}", String::from_utf8_lossy(&out.stderr))));
        }
    }

    let _ = wm.stop_app(&id).await;
    wm.start_app(&pool, &secrets, &id).await?;

    Ok(Json(json!({ "message": format!("integration '{id}' deployed and started") })))
}

pub async fn undeploy(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let (app_dir, pool, secrets, wm) = {
        let g = rt.lock().await;
        let app_dir = g.data_dir().join("apps").join(&id);
        let pool = g.pool().cloned().ok_or(ApiError::NotReady)?;
        let secrets = g.secret_manager().cloned().ok_or(ApiError::NotReady)?;
        let wm = g.worker_manager().cloned().ok_or(ApiError::NotReady)?;
        (app_dir, pool, secrets, wm)
    };

    let _ = wm.stop_app(&id).await;

    let manifest = super::routes::get_integration_manifest(&pool, &id).await?;
    let secret_map = super::routes::platform_secret_map(&manifest);
    for (_, secret_key) in &secret_map {
        let _ = secrets.delete(&pool, "_platform", secret_key).await;
    }

    crate::manifest::uninstall_app(&pool, &id).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if app_dir.exists() {
        tokio::fs::remove_dir_all(&app_dir).await
            .map_err(|e| ApiError::Internal(format!("rm app dir: {e}")))?;
    }

    info!(id = %id, "integration undeployed");
    Ok(Json(json!({ "message": format!("integration '{id}' removed") })))
}

async fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<(), ApiError> {
    tokio::fs::create_dir_all(dst).await.map_err(|e| ApiError::Internal(format!("mkdir: {e}")))?;
    let mut rd = tokio::fs::read_dir(src).await.map_err(|e| ApiError::Internal(format!("readdir: {e}")))?;
    while let Some(entry) = rd.next_entry().await.map_err(|e| ApiError::Internal(e.to_string()))? {
        let dest = dst.join(entry.file_name());
        if entry.file_type().await.map_err(|e| ApiError::Internal(e.to_string()))?.is_dir() {
            Box::pin(copy_dir_recursive(&entry.path(), &dest)).await?;
        } else {
            tokio::fs::copy(entry.path(), &dest).await.map_err(|e| ApiError::Internal(format!("copy: {e}")))?;
        }
    }
    Ok(())
}
