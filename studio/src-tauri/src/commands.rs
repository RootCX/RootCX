use rootcx_shared_types::{AppManifest, ForgeStatus, InstalledApp, OsStatus};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn get_os_status(state: State<'_, AppState>) -> Result<OsStatus, String> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn boot_runtime(state: State<'_, AppState>) -> Result<(), String> {
    state.boot().await
}

#[tauri::command]
pub async fn shutdown_runtime(state: State<'_, AppState>) -> Result<(), String> {
    state.shutdown().await;
    Ok(())
}

#[tauri::command]
pub async fn install_app(
    state: State<'_, AppState>,
    manifest_json: String,
) -> Result<String, String> {
    let manifest: AppManifest =
        serde_json::from_str(&manifest_json).map_err(|e| format!("invalid manifest: {e}"))?;
    state.install_app_manifest(&manifest).await
}

#[tauri::command]
pub async fn get_forge_status(state: State<'_, AppState>) -> Result<ForgeStatus, String> {
    Ok(state.status().await.forge)
}

#[tauri::command]
pub async fn list_apps(state: State<'_, AppState>) -> Result<Vec<InstalledApp>, String> {
    state.list_installed_apps().await
}

#[derive(Serialize)]
pub struct AppLogsResult {
    lines: Vec<String>,
    offset: u64,
}

#[tauri::command]
pub async fn run_app(state: State<'_, AppState>, project_path: String) -> Result<(), String> {
    state.run_app(project_path).await
}

#[tauri::command]
pub async fn stop_app(state: State<'_, AppState>, project_path: String) -> Result<(), String> {
    state.stop_app(project_path).await
}

#[tauri::command]
pub async fn app_logs(
    state: State<'_, AppState>,
    project_path: String,
    since: u64,
) -> Result<AppLogsResult, String> {
    let (lines, offset) = state.app_logs(&project_path, since).await;
    Ok(AppLogsResult { lines, offset })
}

#[tauri::command]
pub async fn list_running_apps(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.running_app_paths().await)
}
