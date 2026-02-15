use rootcx_shared_types::OsStatus;
use serde::Serialize;
use tauri::{ipc::Channel, State};
use tokio::sync::Mutex;

use crate::menu::ViewMenuItems;
use crate::state::AppState;
use crate::terminal::TerminalState;

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

#[tauri::command]
pub fn sync_view_menu(hidden: Vec<String>, state: State<'_, ViewMenuItems>) {
    for (id, item) in &state.0 {
        let _ = item.set_checked(!hidden.contains(id));
    }
}

// ── Filesystem ──

#[tauri::command]
pub async fn read_file(path: String) -> Result<String, String> {
    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("failed to read file: {e}"))
}

#[tauri::command]
pub async fn write_file(path: String, contents: String) -> Result<(), String> {
    tokio::fs::write(&path, contents.as_bytes())
        .await
        .map_err(|e| format!("failed to write file: {e}"))
}

#[tauri::command]
pub async fn scaffold_project(path: String, name: String) -> Result<(), String> {
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|e| format!("failed to create directory: {e}"))?;
    let manifest = serde_json::json!({
        "appId": name.to_lowercase().replace(' ', "-"),
        "name": name,
        "version": "0.0.1",
        "description": "",
        "dataContract": []
    });
    tokio::fs::write(
        format!("{path}/manifest.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .await
    .map_err(|e| format!("failed to write manifest: {e}"))
}

// ── Launch config ──

#[tauri::command]
pub fn read_launch_config(project_path: String) -> Result<crate::launch::LaunchConfig, String> {
    crate::launch::read(std::path::Path::new(&project_path))
}

#[tauri::command]
pub fn init_launch_config(project_path: String) -> Result<(), String> {
    crate::launch::init(std::path::Path::new(&project_path))
}

// ── Terminal ──

#[tauri::command]
pub async fn spawn_terminal(
    cwd: Option<String>,
    rows: u16,
    cols: u16,
    channel: Channel<Vec<u8>>,
    state: State<'_, Mutex<TerminalState>>,
) -> Result<(), String> {
    state.lock().await.spawn(cwd.as_deref(), rows, cols, channel)
}

#[tauri::command]
pub async fn terminal_write(
    data: String,
    state: State<'_, Mutex<TerminalState>>,
) -> Result<(), String> {
    state.lock().await.write(data.as_bytes()).await
}

#[tauri::command]
pub async fn terminal_resize(
    rows: u16,
    cols: u16,
    state: State<'_, Mutex<TerminalState>>,
) -> Result<(), String> {
    state.lock().await.resize(rows, cols).await
}
