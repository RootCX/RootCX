pub mod auth;
mod crons;
pub(crate) mod crud;
mod deploy;
mod introspection;
mod jobs;
pub mod llm_models;
pub(crate) mod query_params;
mod secrets;
mod upload;
mod workers;

use std::sync::Arc;

use axum::Json;
use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;

use crate::ReadyRuntime;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::secrets::SecretManager;
use crate::worker_manager::WorkerManager;
use rootcx_types::{AppManifest, AppType, InstalledApp, OsStatus, SchemaVerification};

pub type SharedRuntime = Arc<ReadyRuntime>;

pub(crate) fn pool(rt: &SharedRuntime) -> PgPool {
    rt.pool().clone()
}

pub(crate) fn wm(rt: &SharedRuntime) -> Arc<WorkerManager> {
    rt.worker_manager().clone()
}

pub(crate) fn pool_and_secrets(rt: &SharedRuntime) -> (PgPool, Arc<SecretManager>) {
    (rt.pool().clone(), rt.secret_manager().clone())
}

fn parse_uuid(id: &str) -> Result<sqlx::types::Uuid, ApiError> {
    id.parse::<sqlx::types::Uuid>().map_err(|_| ApiError::BadRequest(format!("invalid UUID: '{id}'")))
}

pub async fn health() -> Json<JsonValue> {
    Json(json!({ "status": "ok" }))
}

pub async fn get_status(
    _identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
) -> Result<Json<OsStatus>, ApiError> {
    Ok(Json(rt.status()))
}

pub async fn install_app(
    identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
    Json(manifest): Json<AppManifest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt);
    crate::manifest::install_app(&pool, &manifest, rt.extensions(), identity.user_id).await?;
    Ok(Json(json!({ "message": format!("app '{}' installed", manifest.app_id) })))
}

pub async fn list_apps(
    _identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<InstalledApp>>, ApiError> {
    let pool = pool(&rt);
    let data_dir = rt.data_dir().to_path_buf();

    let type_filter = params.get("type").and_then(|t| match t.as_str() {
        "app" => Some(AppType::App),
        "agent" => Some(AppType::Agent),
        "integration" => Some(AppType::Integration),
        _ => None,
    });

    let rows = sqlx::query_as::<_, (String, String, String, String, Option<sqlx::types::JsonValue>, bool)>(
        "SELECT a.id, a.name, a.version, a.status, a.manifest,
                EXISTS(SELECT 1 FROM rootcx_system.agents ag WHERE ag.app_id = a.id) AS is_agent
         FROM rootcx_system.apps a
         WHERE a.status != 'system'
         ORDER BY a.name",
    )
    .fetch_all(&pool)
    .await?;

    let frontends = deploy::list_frontends(&data_dir);

    Ok(Json(rows.into_iter()
        .filter_map(|(id, name, version, status, manifest, is_agent)| {
            let app_type = if is_agent {
                AppType::Agent
            } else if manifest.as_ref().and_then(|m| m.get("type")).and_then(|t| t.as_str()) == Some("integration") {
                AppType::Integration
            } else {
                AppType::App
            };

            if type_filter.is_some_and(|f| f != app_type) { return None; }

            let entities = manifest
                .and_then(|m| m.get("dataContract")?.as_array()
                    .map(|a| a.iter().filter_map(|e| e.get("entityName")?.as_str().map(String::from)).collect()))
                .unwrap_or_default();
            let has_frontend = frontends.contains(&id);
            Some(InstalledApp { id, name, version, status, app_type, entities, has_frontend })
        })
        .collect()))
}

pub async fn uninstall_app(
    _identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
    axum::extract::Path(app_id): axum::extract::Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt);
    let data_dir = rt.data_dir().to_path_buf();
    let _ = wm(&rt).stop_app(&app_id).await;
    crate::manifest::uninstall_app(&pool, &app_id).await?;
    for sub in ["apps", "frontends"] {
        let dir = data_dir.join(sub).join(&app_id);
        if dir.exists() {
            tokio::fs::remove_dir_all(&dir).await
                .map_err(|e| ApiError::Internal(format!("rm {}: {e}", dir.display())))?;
        }
    }
    Ok(Json(json!({ "message": format!("app '{}' uninstalled", app_id) })))
}

pub async fn verify_schema(
    _identity: Identity,
    axum::extract::State(rt): axum::extract::State<SharedRuntime>,
    Json(manifest): Json<AppManifest>,
) -> Result<Json<SchemaVerification>, ApiError> {
    let pool = pool(&rt);
    crate::manifest::validate_manifest(&manifest).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let pk_types = crate::manifest::build_pk_type_map(&manifest.data_contract);
    Ok(Json(crate::schema_sync::verify_all(&pool, &manifest.app_id, &manifest.data_contract, &pk_types).await?))
}

pub use crud::{bulk_create_records, create_record, delete_record, federated_query, get_record, list_records, query_records, update_record};
pub use deploy::{deploy_backend, deploy_frontend, serve_frontend, serve_frontend_root};
pub use jobs::{enqueue_job, list_jobs};
pub use secrets::{delete_secret, list_secrets, set_secret};
pub use secrets::{delete_platform_secret, get_platform_env, list_platform_secrets, set_platform_secret};
pub use upload::upload_file;
pub use introspection::{execute_query, list_schemas, list_tables};
pub use crons::{create_cron, delete_cron, list_all_crons, list_cron_runs, list_crons, trigger_cron, update_cron};
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
