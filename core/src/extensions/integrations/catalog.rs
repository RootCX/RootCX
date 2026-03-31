use std::process::Stdio;

use axum::Json;
use axum::extract::{Path, State};
use serde_json::{Value as JsonValue, json};
use tracing::info;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;

pub async fn list_catalog(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let dir = rt.resources_dir().join("integrations");
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
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let catalog_path = rt.resources_dir().join("integrations").join(&id);
    let app_dir = rt.data_dir().join("apps").join(&id);
    let bun_bin = rt.bun_bin().to_path_buf();
    let pool = rt.pool().clone();
    let secrets = rt.secret_manager().clone();
    let wm = rt.worker_manager().clone();

    if !catalog_path.exists() {
        return Err(ApiError::NotFound(format!("integration '{id}' not in catalog")));
    }

    let manifest_raw = tokio::fs::read_to_string(catalog_path.join("manifest.json"))
        .await
        .map_err(|e| ApiError::Internal(format!("read manifest: {e}")))?;
    let manifest: rootcx_types::AppManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| ApiError::Internal(format!("parse manifest: {e}")))?;

    crate::manifest::install_app(&pool, &manifest, rt.extensions(), identity.user_id).await?;
    sync_integration_permissions(&pool, &id, &manifest).await?;

    sqlx::query("UPDATE rootcx_system.apps SET webhook_token = $1 WHERE id = $2 AND webhook_token IS NULL")
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&id)
        .execute(&pool)
        .await?;

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

    let webhook_token: Option<String> = sqlx::query_scalar(
        "SELECT webhook_token FROM rootcx_system.apps WHERE id = $1",
    ).bind(&id).fetch_optional(&pool).await?.flatten();

    Ok(Json(json!({ "message": format!("integration '{id}' deployed and started"), "webhookToken": webhook_token })))
}

pub async fn undeploy(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let app_dir = rt.data_dir().join("apps").join(&id);
    let pool = rt.pool().clone();
    let secrets = rt.secret_manager().clone();
    let wm = rt.worker_manager().clone();

    let _ = wm.stop_app(&id).await;

    let manifest = super::routes::get_integration_manifest(&pool, &id).await?;
    let secret_map = super::routes::platform_secret_map(&manifest);
    for (_, secret_key) in &secret_map {
        let _ = secrets.delete(&pool, "_platform", secret_key).await;
    }

    sqlx::query(
        "DELETE FROM rootcx_system.rbac_permissions WHERE app_id = 'core' AND key LIKE $1",
    )
    .bind(format!("integration.{id}.%"))
    .execute(&pool)
    .await?;

    crate::manifest::uninstall_app(&pool, &id).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if app_dir.exists() {
        tokio::fs::remove_dir_all(&app_dir).await
            .map_err(|e| ApiError::Internal(format!("rm app dir: {e}")))?;
    }

    info!(id = %id, "integration undeployed");
    Ok(Json(json!({ "message": format!("integration '{id}' removed") })))
}

async fn sync_integration_permissions(
    pool: &sqlx::PgPool,
    integration_id: &str,
    manifest: &rootcx_types::AppManifest,
) -> Result<(), ApiError> {
    if manifest.actions.is_empty() { return Ok(()); }
    let (keys, descs): (Vec<String>, Vec<String>) = manifest.actions.iter().map(|a| (
        format!("integration.{integration_id}.{}", a.id),
        format!("{} via {integration_id}", a.name),
    )).unzip();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_permissions (app_id, key, description)
         SELECT 'core', unnest($1::text[]), unnest($2::text[])
         ON CONFLICT (app_id, key) DO NOTHING",
    )
    .bind(&keys)
    .bind(&descs)
    .execute(pool)
    .await?;
    Ok(())
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
