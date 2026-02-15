use rootcx_shared_types::OsStatus;
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
pub async fn get_forge_status(state: State<'_, AppState>) -> Result<rootcx_shared_types::ForgeStatus, String> {
    Ok(state.status().await.forge)
}

#[derive(Serialize)]
pub struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
}

#[tauri::command]
pub async fn read_dir(path: String) -> Result<Vec<DirEntry>, String> {
    let mut entries = Vec::new();
    let mut rd = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| format!("failed to read directory: {e}"))?;

    while let Some(entry) = rd.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files/dirs
        if name.starts_with('.') {
            continue;
        }
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        entries.push(DirEntry {
            name,
            path: entry.path().to_string_lossy().to_string(),
            is_dir: metadata.is_dir(),
        });
    }

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(entries)
}
