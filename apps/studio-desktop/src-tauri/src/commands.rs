use rootcx_shared_types::{AppManifest, ForgeStatus, InstalledApp, OsStatus};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

/// Returns the current OS status (Kernel + PostgreSQL).
///
/// Called by the React frontend on a polling interval.
#[tauri::command]
pub async fn get_os_status(state: State<'_, AppState>) -> Result<OsStatus, String> {
    Ok(state.status().await)
}

/// Manually trigger the Kernel boot sequence.
#[tauri::command]
pub async fn boot_kernel(state: State<'_, AppState>) -> Result<(), String> {
    state.boot().await.map_err(|e| e.to_string())
}

/// Gracefully shut down the Kernel and PostgreSQL.
#[tauri::command]
pub async fn shutdown_kernel(state: State<'_, AppState>) -> Result<(), String> {
    state.shutdown().await.map_err(|e| e.to_string())
}

/// Install an app from a manifest JSON string.
///
/// Parses the manifest, creates real SQL tables, and registers the app.
#[tauri::command]
pub async fn install_app(
    state: State<'_, AppState>,
    manifest_json: String,
) -> Result<String, String> {
    let manifest: AppManifest =
        serde_json::from_str(&manifest_json).map_err(|e| format!("invalid manifest: {e}"))?;

    let pool = state
        .pool()
        .await
        .ok_or("kernel not booted — no database connection")?;

    rootcx_kernel::install_app(&pool, &manifest)
        .await
        .map_err(|e| e.to_string())?;

    Ok(format!("app '{}' installed successfully", manifest.app_id))
}

/// Returns the AI Forge sidecar status.
#[tauri::command]
pub async fn get_forge_status(state: State<'_, AppState>) -> Result<ForgeStatus, String> {
    let status = state.status().await;
    Ok(status.forge)
}

/// List all installed apps.
#[tauri::command]
pub async fn list_apps(state: State<'_, AppState>) -> Result<Vec<InstalledApp>, String> {
    let pool = state
        .pool()
        .await
        .ok_or("kernel not booted — no database connection")?;

    let rows = sqlx::query_as::<_, (String, String, String, String, Option<sqlx::types::JsonValue>)>(
        r#"
        SELECT id, name, version, status, manifest
        FROM rootcx_system.apps
        ORDER BY name
        "#,
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| e.to_string())?;

    let apps: Vec<InstalledApp> = rows
        .into_iter()
        .map(|(id, name, version, status, manifest)| {
            let entities = manifest
                .and_then(|m| {
                    m.get("dataContract")
                        .and_then(|dc| dc.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    e.get("entityName").and_then(|n| n.as_str()).map(String::from)
                                })
                                .collect::<Vec<_>>()
                        })
                })
                .unwrap_or_default();

            InstalledApp {
                id,
                name,
                version,
                status,
                entities,
            }
        })
        .collect();

    Ok(apps)
}

// ── App Runner commands ─────────────────────────────

#[derive(Serialize)]
pub struct AppLogsResult {
    lines: Vec<String>,
    offset: u64,
}

/// Start a Forge-built app via `cargo tauri dev`.
#[tauri::command]
pub async fn run_app(state: State<'_, AppState>, project_path: String) -> Result<(), String> {
    state.run_app(project_path).await
}

/// Stop a running app.
#[tauri::command]
pub async fn stop_app(state: State<'_, AppState>, project_path: String) -> Result<(), String> {
    state.stop_app(project_path).await
}

/// Poll for new log lines from a running app.
#[tauri::command]
pub async fn app_logs(
    state: State<'_, AppState>,
    project_path: String,
    since: u64,
) -> Result<AppLogsResult, String> {
    let (lines, offset) = state.app_logs(&project_path, since).await;
    Ok(AppLogsResult { lines, offset })
}

/// List all currently running app project paths.
#[tauri::command]
pub async fn list_running_apps(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.running_app_paths().await)
}
