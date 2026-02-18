use rootcx_shared_types::OsStatus;
use serde::Serialize;
use tauri::{ipc::Channel, State};
use tokio::sync::Mutex;

use crate::menu::ViewMenuItems;
use crate::runner::RunnerState;
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

#[tauri::command]
pub async fn start_forge(state: State<'_, AppState>, project_path: String) -> Result<(), String> {
    state.start_forge(&project_path).await
}

#[tauri::command]
pub async fn save_forge_config(
    state: State<'_, AppState>,
    contents: String,
    project_path: Option<String>,
) -> Result<(), String> {
    state
        .save_forge_config(&contents, project_path.as_deref())
        .await
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
pub async fn ensure_dir(path: String) -> Result<(), String> {
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|e| format!("failed to create directory: {e}"))
}

#[tauri::command]
pub async fn sync_manifest(
    state: State<'_, AppState>,
    project_path: String,
) -> Result<(), String> {
    state.sync_manifest(&project_path).await
}

#[tauri::command]
pub async fn deploy_backend(
    state: State<'_, AppState>,
    project_path: String,
) -> Result<String, String> {
    state.deploy_and_watch(&project_path).await
}

#[tauri::command]
pub async fn run_app(
    command: String,
    project_path: String,
    app_handle: tauri::AppHandle,
    state: State<'_, Mutex<RunnerState>>,
) -> Result<(), String> {
    state.lock().await.run(&command, &project_path, app_handle);
    Ok(())
}

#[tauri::command]
pub async fn stop_deployed_worker(state: State<'_, AppState>) -> Result<(), String> {
    state.stop_deployed_worker().await;
    Ok(())
}

#[tauri::command]
pub async fn scaffold_project(path: String, name: String) -> Result<(), String> {
    let sdk = crate::state::sdk_runtime_dir()?;
    let client_crate = crate::state::runtime_client_dir()?;
    crate::scaffold::create(std::path::Path::new(&path), &name, &sdk, &client_crate).await
}

#[tauri::command]
pub async fn resolve_instructions() -> Result<Vec<String>, String> {
    let dir = crate::state::instructions_dir()?;
    let mut rd = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(_) => return Ok(vec![]),
    };
    let mut files = Vec::new();
    while let Ok(Some(entry)) = rd.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".md") {
            files.push(name);
        }
    }
    files.sort();
    Ok(files)
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
