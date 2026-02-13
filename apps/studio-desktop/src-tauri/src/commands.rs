use rootcx_shared_types::OsStatus;
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
