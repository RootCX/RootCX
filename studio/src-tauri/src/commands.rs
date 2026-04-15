use rootcx_types::OsStatus;
use serde::Serialize;
use tauri::{State, ipc::Channel};
use tauri::webview::WebviewWindowBuilder;
use tokio::sync::Mutex;

use crate::menu::ViewMenuItems;
use crate::runner::RunnerState;
use crate::state::AppState;
use crate::terminal::TerminalState;

#[tauri::command]
pub fn get_core_url(state: State<'_, AppState>) -> String {
    state.core_url()
}

#[tauri::command]
pub fn set_core_url(state: State<'_, AppState>, url: String) {
    state.set_core_url(&url);
}

#[tauri::command]
pub fn set_auth_token(state: State<'_, AppState>, token: String) {
    state.set_auth_token(if token.is_empty() { None } else { Some(token) });
}

#[tauri::command]
pub async fn get_os_status(state: State<'_, AppState>) -> Result<OsStatus, String> {
    Ok(state.status().await)
}

#[tauri::command]
pub async fn boot_runtime(state: State<'_, AppState>) -> Result<(), String> {
    state.boot().await
}

#[tauri::command]
pub async fn await_boot(rx: State<'_, tokio::sync::watch::Receiver<bool>>) -> Result<(), String> {
    let mut rx = rx.inner().clone();
    rx.wait_for(|&ready| ready).await.map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn shutdown_runtime(state: State<'_, AppState>) -> Result<(), String> {
    state.shutdown().await;
    Ok(())
}

#[derive(Serialize)]
pub struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
}

#[tauri::command]
pub async fn read_dir(path: String) -> Result<Vec<DirEntry>, String> {
    validate_fs_path(&path)?;
    let mut entries = Vec::new();
    let mut rd = tokio::fs::read_dir(&path).await.map_err(|e| format!("failed to read directory: {e}"))?;

    while let Some(entry) = rd.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let metadata = entry.metadata().await.map_err(|e| e.to_string())?;
        entries.push(DirEntry { name, path: entry.path().to_string_lossy().to_string(), is_dir: metadata.is_dir() });
    }

    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));

    Ok(entries)
}

#[tauri::command]
pub fn sync_view_menu(hidden: Vec<String>, state: State<'_, ViewMenuItems>) {
    for (id, item) in &state.0 {
        let _ = item.set_checked(!hidden.contains(id));
    }
}

pub(crate) fn validate_fs_path(path: &str) -> Result<(), String> {
    let home = rootcx_platform::dirs::home_dir().map_err(|e| e.to_string())?;
    let raw = std::path::Path::new(path);

    if !raw.is_absolute() {
        return Err("path must be absolute".into());
    }

    let resolved = normalize_lexical(raw);
    let home = normalize_lexical(&home);

    if !resolved.starts_with(&home) {
        return Err("path must be under home directory".into());
    }

    let rel = resolved.strip_prefix(&home).unwrap();
    const BLOCKED: &[&str] = &[".ssh", ".gnupg", ".aws", ".config/gcloud", ".kube"];
    if BLOCKED.iter().any(|b| rel.starts_with(b)) {
        let first = rel.components().next().and_then(|c| c.as_os_str().to_str()).unwrap_or("");
        return Err(format!("access to ~/{first} is blocked"));
    }

    Ok(())
}

fn normalize_lexical(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            _ => out.push(comp),
        }
    }
    out
}

#[tauri::command]
pub async fn read_file(path: String) -> Result<String, String> {
    validate_fs_path(&path)?;
    tokio::fs::read_to_string(&path).await.map_err(|e| format!("failed to read file: {e}"))
}

#[tauri::command]
pub async fn write_file(path: String, contents: String) -> Result<(), String> {
    validate_fs_path(&path)?;
    tokio::fs::write(&path, contents.as_bytes()).await.map_err(|e| format!("failed to write file: {e}"))
}

#[tauri::command]
pub async fn ensure_dir(path: String) -> Result<(), String> {
    validate_fs_path(&path)?;
    tokio::fs::create_dir_all(&path).await.map_err(|e| format!("failed to create directory: {e}"))
}

#[tauri::command]
pub async fn install_deps(state: State<'_, AppState>, project_path: String) -> Result<(), String> {
    validate_fs_path(&project_path)?;
    state.install_deps(&project_path).await
}

#[tauri::command]
pub async fn deploy_backend(state: State<'_, AppState>, project_path: String) -> Result<String, String> {
    validate_fs_path(&project_path)?;
    state.deploy_and_watch(&project_path).await
}

#[tauri::command]
pub async fn deploy_frontend(state: State<'_, AppState>, project_path: String) -> Result<String, String> {
    validate_fs_path(&project_path)?;
    state.publish_frontend(&project_path).await
}

#[tauri::command]
pub async fn run_app(
    project_path: String,
    app_handle: tauri::AppHandle,
    state: State<'_, Mutex<RunnerState>>,
    app_state: State<'_, AppState>,
) -> Result<(), String> {
    validate_fs_path(&project_path)?;
    let config = crate::launch::read(std::path::Path::new(&project_path))?;
    if let Some(ref cmd) = config.command {
        state.lock().await.run(cmd, &project_path, &app_state.core_url(), app_handle);
    }
    Ok(())
}

#[tauri::command]
pub async fn stop_deployed_worker(state: State<'_, AppState>) -> Result<(), String> {
    state.stop_deployed_worker().await;
    Ok(())
}

#[tauri::command]
pub async fn scaffold_project(
    _state: State<'_, AppState>,
    path: String,
    name: String,
    preset_id: Option<String>,
    answers: Option<std::collections::HashMap<String, crate::scaffold::Answer>>,
) -> Result<(), String> {
    validate_fs_path(&path)?;
    crate::scaffold::create(
        std::path::Path::new(&path),
        &name,
        preset_id.as_deref().unwrap_or("blank"),
        answers.unwrap_or_default(),
    )
    .await
}

#[tauri::command]
pub fn list_presets() -> Vec<crate::scaffold::PresetInfo> {
    crate::scaffold::Registry::new().list()
}

#[tauri::command]
pub fn get_preset_questions(preset_id: String) -> Result<Vec<crate::scaffold::Question>, String> {
    crate::scaffold::Registry::new().questions(&preset_id)
}

#[tauri::command]
pub async fn resolve_skills() -> Result<Vec<serde_json::Value>, String> {
    let dirs = crate::state::skills_dirs();
    let entries = rootcx_forge::skills::discover(&dirs).await;
    Ok(entries.iter().map(|s| serde_json::json!({
        "name": s.name,
        "description": s.description,
        "path": s.path.display().to_string(),
    })).collect())
}

#[tauri::command]
pub fn read_launch_config(project_path: String) -> Result<crate::launch::LaunchConfig, String> {
    crate::launch::read(std::path::Path::new(&project_path))
}

#[tauri::command]
pub fn init_launch_config(project_path: String) -> Result<(), String> {
    crate::launch::init(std::path::Path::new(&project_path))
}

#[tauri::command]
pub async fn spawn_terminal(
    cwd: Option<String>,
    rows: u16,
    cols: u16,
    channel: Channel<Vec<u8>>,
    state: State<'_, Mutex<TerminalState>>,
) -> Result<(), String> {
    if let Some(ref dir) = cwd {
        validate_fs_path(dir)?;
    }
    state.lock().await.spawn(cwd.as_deref(), rows, cols, channel)
}

#[tauri::command]
pub async fn terminal_write(data: String, state: State<'_, Mutex<TerminalState>>) -> Result<(), String> {
    state.lock().await.write(data.as_bytes()).await
}

#[tauri::command]
pub async fn terminal_resize(rows: u16, cols: u16, state: State<'_, Mutex<TerminalState>>) -> Result<(), String> {
    state.lock().await.resize(rows, cols).await
}

#[tauri::command]
pub fn create_window(app_handle: tauri::AppHandle, project_path: Option<String>) -> Result<String, String> {
    let label = format!("studio-{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let url_path = match &project_path {
        Some(path) => format!("index.html?project={}", urlencoding::encode(path)),
        None => "index.html".to_string(),
    };

    let window = WebviewWindowBuilder::new(&app_handle, &label, tauri::WebviewUrl::App(url_path.into()))
        .title("RootCX Studio")
        .inner_size(1024.0, 700.0)
        .resizable(true)
        .build()
        .map_err(|e| format!("failed to create window: {e}"))?;

    crate::menu::track_window_focus(&window);

    Ok(label)
}

#[tauri::command]
pub async fn bundle_app(
    project_path: String,
    app_handle: tauri::AppHandle,
    runner: State<'_, Mutex<RunnerState>>,
) -> Result<(), String> {
    use tauri::Emitter;
    validate_fs_path(&project_path)?;

    runner.lock().await.stop();
    let _ = app_handle.emit("run-started", ());

    tokio::task::spawn_blocking(move || {
        let handle = app_handle.clone();
        let log = move |msg: &str| { let _ = handle.emit("run-output", format!("{msg}\r\n")); };
        match rootcx_platform::bundle::run(project_path.into(), &log) {
            Ok(path) => {
                log(&format!("[bundle] done → {}", path.display()));
                let _ = app_handle.emit("run-exited", Some(0i32));
                Ok(())
            }
            Err(e) => {
                log(&format!("[bundle] error: {e}"));
                let _ = app_handle.emit("run-exited", Some(1i32));
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| format!("bundle task: {e}"))?
}

#[tauri::command]
pub fn get_recent_projects(state: State<'_, AppState>) -> Vec<crate::state::RecentProject> {
    state.recent_projects()
}

#[tauri::command]
pub fn add_to_recent(app_handle: tauri::AppHandle, project_path: String, state: State<'_, AppState>) {
    state.add_recent_project(&project_path);
    crate::menu::rebuild_recent_menu(&app_handle, &state.recent_projects());
}

#[tauri::command]
pub fn clear_recent(app_handle: tauri::AppHandle, state: State<'_, AppState>) {
    state.clear_recent_projects();
    crate::menu::rebuild_recent_menu(&app_handle, &[]);
}

const COMPOSE_YAML: &str = r#"services:
  postgres:
    image: ghcr.io/rootcx/postgresql:16-pgmq
    user: root
    entrypoint: ["/pg-entrypoint.sh"]
    environment:
      POSTGRES_USER: rootcx
      POSTGRES_PASSWORD: rootcx
      POSTGRES_DB: rootcx
      PGDATA: /data/pgdata
    volumes:
      - pgdata:/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U rootcx -d rootcx"]
      interval: 2s
      timeout: 5s
      retries: 10
  core:
    image: ghcr.io/rootcx/core:latest
    depends_on:
      postgres:
        condition: service_healthy
    environment:
      DATABASE_URL: postgres://rootcx:rootcx@postgres:5432/rootcx
    ports:
      - "9100:9100"
    volumes:
      - data:/data
volumes:
  pgdata:
  data:
"#;

#[tauri::command]
pub async fn check_docker() -> Result<bool, String> {
    let out = tokio::process::Command::new("docker").arg("info")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().await;
    Ok(out.map(|s| s.success()).unwrap_or(false))
}

#[tauri::command]
pub async fn start_local_core() -> Result<(), String> {
    let dir = std::env::temp_dir().join("rootcx-compose");
    std::fs::create_dir_all(&dir).map_err(|e| format!("tmpdir: {e}"))?;
    let file = dir.join("docker-compose.yml");
    std::fs::write(&file, COMPOSE_YAML).map_err(|e| format!("write compose: {e}"))?;

    let out = tokio::process::Command::new("docker")
        .args(["compose", "-f", &file.to_string_lossy(), "up", "-d", "--wait"])
        .output().await.map_err(|e| format!("docker compose: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("docker compose up failed: {stderr}"));
    }

    let client = reqwest::Client::new();
    for _ in 0..90 {
        if client.get("http://localhost:9100/health")
            .timeout(std::time::Duration::from_secs(2))
            .send().await.map(|r| r.status().is_success()).unwrap_or(false)
        { return Ok(()); }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err("core started but health check timed out after 90s".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fs_path_blocks_sensitive_dirs() {
        let home = rootcx_platform::dirs::home_dir().unwrap().to_string_lossy().to_string();
        for dir in [".ssh", ".gnupg", ".aws", ".kube"] {
            let path = format!("{home}/{dir}/id_rsa");
            assert!(validate_fs_path(&path).is_err(), "should block: {path}");
        }
    }

    #[test]
    fn fs_path_blocks_outside_home() {
        assert!(validate_fs_path("/etc/passwd").is_err());
        assert!(validate_fs_path("/tmp/evil").is_err());
    }

    #[test]
    fn fs_path_blocks_relative() {
        assert!(validate_fs_path("relative/path").is_err());
    }

    #[test]
    fn fs_path_allows_project_dirs() {
        let home = rootcx_platform::dirs::home_dir().unwrap().to_string_lossy().to_string();
        assert!(validate_fs_path(&format!("{home}/workspace/project/src/main.rs")).is_ok());
        assert!(validate_fs_path(&format!("{home}/Documents/file.txt")).is_ok());
    }

    #[test]
    fn fs_path_blocks_dotdot_traversal_to_sensitive_dirs() {
        let home = rootcx_platform::dirs::home_dir().unwrap().to_string_lossy().to_string();
        let traversal = format!("{home}/workspace/../.ssh/id_rsa");
        assert!(validate_fs_path(&traversal).is_err(), "should block ../ traversal to .ssh");
    }
}
