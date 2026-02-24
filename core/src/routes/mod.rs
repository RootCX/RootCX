pub mod auth;
mod config;
mod crud;
mod deploy;
mod jobs;
mod secrets;
mod upload;
mod workers;

use std::sync::Arc;

use axum::Json;
use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use tokio::sync::Mutex;

use crate::Runtime;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::secrets::SecretManager;
use crate::worker_manager::WorkerManager;
use rootcx_shared_types::{AppManifest, InstalledApp, OsStatus, SchemaVerification};

pub type SharedRuntime = Arc<Mutex<Runtime>>;

pub(crate) async fn pool(rt: &SharedRuntime) -> Result<PgPool, ApiError> {
    rt.lock().await.pool().cloned().ok_or(ApiError::NotReady)
}

pub(crate) async fn wm(rt: &SharedRuntime) -> Result<Arc<WorkerManager>, ApiError> {
    rt.lock().await.worker_manager().cloned().ok_or(ApiError::NotReady)
}

async fn pool_and_secrets(rt: &SharedRuntime) -> Result<(PgPool, Arc<SecretManager>), ApiError> {
    let g = rt.lock().await;
    Ok((g.pool().cloned().ok_or(ApiError::NotReady)?, g.secret_manager().cloned().ok_or(ApiError::NotReady)?))
}

fn parse_uuid(id: &str) -> Result<sqlx::types::Uuid, ApiError> {
    id.parse::<sqlx::types::Uuid>().map_err(|_| ApiError::BadRequest(format!("invalid UUID: '{id}'")))
}

pub async fn health() -> Json<JsonValue> {
    Json(json!({ "status": "ok" }))
}

pub async fn get_status(
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
) -> Result<Json<OsStatus>, ApiError> {
    Ok(Json(rt.lock().await.status().await))
}

pub async fn install_app(
    _identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
    Json(manifest): Json<AppManifest>,
) -> Result<Json<JsonValue>, ApiError> {
    let g = rt.lock().await;
    let pool = g.pool().cloned().ok_or(ApiError::NotReady)?;
    crate::manifest::install_app(&pool, &manifest, g.extensions()).await?;
    Ok(Json(json!({ "message": format!("app '{}' installed", manifest.app_id) })))
}

pub async fn list_apps(
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
) -> Result<Json<Vec<InstalledApp>>, ApiError> {
    let pool = pool(&rt).await?;
    let rows = sqlx::query_as::<_, (String, String, String, String, Option<sqlx::types::JsonValue>)>(
        "SELECT id, name, version, status, manifest FROM rootcx_system.apps ORDER BY name",
    )
    .fetch_all(&pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, name, version, status, manifest)| {
                let entities = manifest
                    .and_then(|m| {
                        m.get("dataContract")?
                            .as_array()
                            .map(|a| a.iter().filter_map(|e| e.get("entityName")?.as_str().map(String::from)).collect())
                    })
                    .unwrap_or_default();
                InstalledApp { id, name, version, status, entities }
            })
            .collect(),
    ))
}

pub async fn uninstall_app(
    _identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
    axum::extract::Path(app_id): axum::extract::Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    if let Ok(w) = wm(&rt).await {
        let _ = w.stop_app(&app_id).await;
    }
    let pool = pool(&rt).await?;
    crate::manifest::uninstall_app(&pool, &app_id).await?;
    Ok(Json(json!({ "message": format!("app '{}' uninstalled", app_id) })))
}

pub async fn verify_schema(
    _identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
    Json(manifest): Json<AppManifest>,
) -> Result<Json<SchemaVerification>, ApiError> {
    let pool = pool(&rt).await?;
    let pk_types = crate::manifest::build_pk_type_map(&manifest.data_contract);
    Ok(Json(crate::schema_sync::verify_all(&pool, &manifest.app_id, &manifest.data_contract, &pk_types).await?))
}

pub use config::{get_ai_config, get_forge_config, set_ai_config};
pub use crud::{create_record, delete_record, get_record, list_records, query_records, update_record};
pub use deploy::deploy_backend;
pub use jobs::{enqueue_job, get_job, list_jobs};
pub use secrets::{delete_secret, list_secrets, set_secret};
pub use secrets::{delete_platform_secret, get_platform_env, list_platform_secrets, set_platform_secret};
pub use upload::upload_file;
pub use workers::{all_worker_statuses, rpc_proxy, start_worker, stop_worker, worker_status};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uuid_rejects_invalid() {
        for input in ["not-a-uuid", "", "550e8400-e29b-41d4-a716-44665544000g"] {
            let err = parse_uuid(input).unwrap_err();
            assert!(format!("{err:?}").contains("invalid UUID"), "input: {input:?}");
        }
    }
}
