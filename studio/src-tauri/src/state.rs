use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use rootcx_client::RuntimeClient;
use rootcx_types::OsStatus;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tracing::{info, warn};

pub(crate) const DAEMON_URL: &str = "http://localhost:9100";
const _: () = assert!(rootcx_platform::DEFAULT_API_PORT == 9100, "DAEMON_URL must match DEFAULT_API_PORT");

static LOG_HTTP: LazyLock<reqwest::Client> =
    LazyLock::new(|| reqwest::Client::new());

#[derive(Clone)]
pub struct AppState {
    client: RuntimeClient,
    watchers: Arc<Mutex<Vec<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>>,
    active_app_id: Arc<Mutex<Option<String>>>,
    app_handle: AppHandle,
    log_stream_abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

pub fn config_dir() -> Result<PathBuf, String> { rootcx_platform::dirs::config_dir().map_err(|e| e.to_string()) }

pub fn instructions_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("instructions"))
}

fn session_file() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("studio-session.json"))
}

fn recent_projects_file() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("studio-recent.json"))
}

const MAX_RECENT: usize = 10;

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    pub last_opened: i64,
}

pub fn load_recent_projects() -> Vec<RecentProject> {
    recent_projects_file()
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default()
}

pub fn add_recent_project(project_path: &str) {
    let mut recents = load_recent_projects();
    recents.retain(|r| r.path != project_path);

    let name = Path::new(project_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| project_path.to_string());

    recents.insert(0, RecentProject { path: project_path.to_string(), name, last_opened: unix_now() });
    recents.truncate(MAX_RECENT);

    if let Ok(file) = recent_projects_file() {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&file, serde_json::to_string_pretty(&recents).unwrap_or_default());
    }
}

pub fn clear_recent_projects() {
    if let Ok(file) = recent_projects_file() {
        let _ = std::fs::remove_file(&file);
    }
}

async fn read_manifest(project_path: &str) -> Result<rootcx_types::AppManifest, String> {
    let contents = tokio::fs::read_to_string(Path::new(project_path).join("manifest.json"))
        .await
        .map_err(|e| format!("failed to read manifest: {e}"))?;
    serde_json::from_str(&contents).map_err(|e| format!("invalid manifest: {e}"))
}

const DEPLOY_EXCLUDE: &[&str] = &["node_modules", ".git", ".rootcx", "bun.lock", "src-tauri"];

impl AppState {
    pub fn from_tauri(app: &tauri::App) -> Self {
        Self {
            client: RuntimeClient::new(DAEMON_URL),
            watchers: Arc::new(Mutex::new(Vec::new())),
            active_app_id: Arc::new(Mutex::new(None)),
            app_handle: app.handle().clone(),
            log_stream_abort: Arc::new(Mutex::new(None)),
        }
    }

    pub fn runtime_client(&self) -> &RuntimeClient {
        &self.client
    }

    pub fn set_auth_token(&self, token: Option<String>) {
        self.client.set_token(token);
    }

    pub async fn boot(&self) -> Result<(), String> {
        if !self.client.is_available().await {
            return Err(format!("core daemon not reachable at {DAEMON_URL}"));
        }
        info!("connected to core daemon at {DAEMON_URL}");

        self.reconnect_or_cleanup().await;
        crate::browser::spawn_listener();
        let _ = self.app_handle.emit("runtime-booted", ());
        Ok(())
    }

    async fn reconnect_or_cleanup(&self) {
        let path = match session_file() {
            Ok(p) => p,
            Err(_) => return,
        };
        let data = match tokio::fs::read_to_string(&path).await {
            Ok(d) => d,
            Err(_) => return,
        };
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&data)
            && let Some(app_id) = parsed["active_app_id"].as_str() {
                match self.client.worker_status(app_id).await {
                    Ok(status) if status == "running" => {
                        info!(app_id, "reconnecting to running worker from previous session");
                        *self.active_app_id.lock().await = Some(app_id.to_string());
                        self.subscribe_to_worker_logs(app_id).await;
                        return;
                    }
                    _ => {
                        info!(app_id, "cleaning up orphaned worker from previous session");
                        if let Err(e) = self.client.stop_worker(app_id).await {
                            warn!(app_id, "orphan cleanup failed (may already be stopped): {e}");
                        }
                    }
                }
            }
        let _ = tokio::fs::remove_file(&path).await;
    }

    async fn subscribe_to_worker_logs(&self, app_id: &str) {
        if let Some(handle) = self.log_stream_abort.lock().await.take() {
            handle.abort();
        }

        let app_handle = self.app_handle.clone();
        let _ = app_handle.emit("run-started", ());

        let url = format!("{DAEMON_URL}/api/v1/apps/{app_id}/logs");
        let abort_store = self.log_stream_abort.clone();

        let task = tokio::spawn(async move {
            let client = LOG_HTTP.clone();
            loop {
                match client.get(&url).send().await {
                    Ok(mut resp) if resp.status().is_success() => {
                        let mut buf = String::new();
                        loop {
                            match resp.chunk().await {
                                Ok(Some(chunk)) => {
                                    buf.push_str(&String::from_utf8_lossy(&chunk));
                                    let mut consumed = 0;
                                    while let Some(pos) = buf[consumed..].find('\n') {
                                        let end = consumed + pos;
                                        let line = buf[consumed..end].trim_end_matches('\r');
                                        if let Some(data) = line.strip_prefix("data:")
                                            && let Ok(entry) =
                                                serde_json::from_str::<serde_json::Value>(data.trim_start())
                                            {
                                                let level = entry["level"].as_str().unwrap_or("info");
                                                let message = entry["message"].as_str().unwrap_or("");
                                                let _ = app_handle.emit("run-output", format_log_line(level, message));
                                            }
                                        consumed = end + 1;
                                    }
                                    if consumed > 0 {
                                        buf.drain(..consumed);
                                    }
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    warn!("log stream chunk error: {e}");
                                    break;
                                }
                            }
                        }
                        info!("log stream disconnected, reconnecting...");
                    }
                    Ok(resp) => warn!(status = %resp.status(), "log stream request failed"),
                    Err(e) => warn!("log stream connection failed: {e}"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });

        *abort_store.lock().await = Some(task.abort_handle());
    }

    pub async fn get_ai_config(&self) -> Result<Option<rootcx_types::AiConfig>, String> {
        self.client.get_ai_config().await.map_err(|e| format!("failed to get AI config: {e}"))
    }

    pub async fn deploy_backend(&self, project_path: &str) -> Result<String, String> {
        let project = Path::new(project_path);
        let manifest = read_manifest(project_path).await?;
        let app_id = manifest.app_id;

        let deploy_dir = project.join("backend");
        if !deploy_dir.exists() {
            return Err("no backend/ directory found in project".into());
        }

        let archive = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            for entry in std::fs::read_dir(&deploy_dir).map_err(|e| format!("read dir: {e}"))? {
                let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if DEPLOY_EXCLUDE.contains(&name_str.as_ref()) {
                    continue;
                }
                let path = entry.path();
                if path.is_file() {
                    tar.append_path_with_name(&path, &*name_str).map_err(|e| format!("tar: {e}"))?;
                } else if path.is_dir() {
                    tar.append_dir_all(&*name_str, &path).map_err(|e| format!("tar: {e}"))?;
                }
            }
            tar.into_inner()
                .map_err(|e| format!("tar finalize: {e}"))?
                .finish()
                .map_err(|e| format!("gzip: {e}"))
        })
        .await
        .map_err(|e| format!("archive task failed: {e}"))??;

        info!(app_id = %app_id, size = archive.len(), "deploying backend");
        let result = self.client.deploy_app(&app_id, archive).await.map_err(|e| format!("deploy failed: {e}"))?;

        *self.active_app_id.lock().await = Some(app_id.clone());
        if let Ok(path) = session_file() {
            let data = serde_json::json!({ "active_app_id": app_id });
            let _ = tokio::fs::write(&path, data.to_string()).await;
        }

        Ok(result)
    }

    pub async fn deploy_and_watch(&self, project_path: &str) -> Result<String, String> {
        self.watchers.lock().await.clear();
        let msg = self.deploy_backend(project_path).await?;
        if let Some(app_id) = self.active_app_id.lock().await.clone() {
            self.subscribe_to_worker_logs(&app_id).await;
        }
        self.start_watcher(project_path)?;
        Ok(msg)
    }

    fn start_watcher(&self, project_path: &str) -> Result<(), String> {
        let project = Path::new(project_path);
        let watch_dir = project.join("backend");
        if !watch_dir.exists() {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = new_debouncer(std::time::Duration::from_millis(500), tx)
            .map_err(|e| format!("watcher setup failed: {e}"))?;
        debouncer
            .watcher()
            .watch(&watch_dir, notify::RecursiveMode::Recursive)
            .map_err(|e| format!("watch failed: {e}"))?;

        info!(dir = %watch_dir.display(), "watching for hot reload");

        let state = self.clone();
        let project_path = project_path.to_string();
        let handle = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            while let Ok(Ok(events)) = rx.recv() {
                if events.iter().any(|e| e.kind == DebouncedEventKind::Any) {
                    info!("backend changed, redeploying");
                    match handle.block_on(state.deploy_backend(&project_path)) {
                        Ok(_) => {
                            info!("hot reload success");
                            if let Some(app_id) = handle.block_on(state.active_app_id.lock()).clone() {
                                handle.block_on(state.subscribe_to_worker_logs(&app_id));
                            }
                        }
                        Err(e) => warn!("hot reload failed: {e}"),
                    }
                }
            }
        });

        let watchers = self.watchers.clone();
        tokio::spawn(async move { watchers.lock().await.push(debouncer) });
        Ok(())
    }

    pub async fn stop_deployed_worker(&self) {
        if let Some(handle) = self.log_stream_abort.lock().await.take() {
            handle.abort();
        }
        if let Some(app_id) = self.active_app_id.lock().await.take() {
            info!(app_id, "stopping deployed worker");
            if let Err(e) = self.client.stop_worker(&app_id).await {
                warn!(app_id, "failed to stop worker: {e}");
            }
        }
        let _ = self.app_handle.emit("run-output", "\r\n[worker stopped]\r\n");
        if let Ok(path) = session_file() {
            let _ = tokio::fs::remove_file(&path).await;
        }
        self.watchers.lock().await.clear();
    }

    pub async fn shutdown(&self) {
        crate::browser::shutdown().await;
        self.stop_deployed_worker().await;
    }

    pub async fn status(&self) -> OsStatus {
        self.client.status().await.unwrap_or_else(|_| OsStatus::offline())
    }
}

fn format_log_line(level: &str, message: &str) -> String {
    let (prefix, reset) = match level {
        "error" | "stderr" => ("\x1b[31m", "\x1b[0m"),
        "warn" => ("\x1b[33m", "\x1b[0m"),
        "system" => ("\x1b[36m", "\x1b[0m"),
        "debug" => ("\x1b[90m", "\x1b[0m"),
        _ => ("", ""),
    };
    format!("{prefix}{message}{reset}\r\n")
}
