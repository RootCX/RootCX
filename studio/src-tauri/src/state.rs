use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use rootcx_client::RuntimeClient;
use rootcx_types::OsStatus;
use tauri::{AppHandle, Emitter};
use tauri_plugin_store::StoreExt;
use tokio::sync::Mutex;
use tracing::{info, warn};

pub const STORE_FILE: &str = "studio.json";
const DEFAULT_CORE_URL: &str = "http://localhost:9100";
const MAX_RECENT: usize = 10;
const DEPLOY_EXCLUDE: &[&str] = &["node_modules", ".git", ".rootcx", "bun.lock", "src-tauri"];

static LOG_HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

pub fn config_dir() -> Result<PathBuf, String> { rootcx_platform::dirs::config_dir().map_err(|e| e.to_string()) }

pub fn instructions_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("instructions"))
}

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

async fn read_manifest(project_path: &str) -> Result<rootcx_types::AppManifest, String> {
    let contents = tokio::fs::read_to_string(Path::new(project_path).join("manifest.json"))
        .await
        .map_err(|e| format!("failed to read manifest: {e}"))?;
    serde_json::from_str(&contents).map_err(|e| format!("invalid manifest: {e}"))
}

#[derive(Clone)]
pub struct AppState {
    client: Arc<std::sync::RwLock<RuntimeClient>>,
    watchers: Arc<Mutex<Vec<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>>,
    app_handle: AppHandle,
    log_stream_abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

impl AppState {
    pub fn from_tauri(app: &tauri::App) -> Self {
        let url = app.store(STORE_FILE)
            .ok()
            .and_then(|s| s.get("core_url"))
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| DEFAULT_CORE_URL.to_string());
        Self {
            client: Arc::new(std::sync::RwLock::new(RuntimeClient::new(&url))),
            watchers: Arc::new(Mutex::new(Vec::new())),
            app_handle: app.handle().clone(),
            log_stream_abort: Arc::new(Mutex::new(None)),
        }
    }

    fn store(&self) -> Arc<tauri_plugin_store::Store<tauri::Wry>> {
        self.app_handle.store(STORE_FILE).expect("store")
    }

    pub fn core_url(&self) -> String {
        self.store().get("core_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| DEFAULT_CORE_URL.to_string())
    }

    pub fn set_core_url(&self, url: &str) {
        let url = url.trim_end_matches('/');
        self.store().set("core_url", url);
        *self.client.write().unwrap() = RuntimeClient::new(url);
    }

    pub fn is_remote(&self) -> bool {
        let url = self.core_url();
        !url.contains("localhost") && !url.contains("127.0.0.1")
    }

    pub fn client(&self) -> RuntimeClient {
        self.client.read().unwrap().clone()
    }

    pub fn set_auth_token(&self, token: Option<String>) {
        self.client().set_token(token);
    }

    pub fn recent_projects(&self) -> Vec<RecentProject> {
        self.store().get("recent_projects")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }

    pub fn add_recent_project(&self, path: &str) {
        let mut recents = self.recent_projects();
        recents.retain(|r| r.path != path);
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        recents.insert(0, RecentProject { path: path.to_string(), name, last_opened: unix_now() });
        recents.truncate(MAX_RECENT);
        self.store().set("recent_projects", serde_json::to_value(&recents).unwrap());
    }

    pub fn clear_recent_projects(&self) {
        self.store().delete("recent_projects");
    }

    fn active_app_id(&self) -> Option<String> {
        self.store().get("active_app_id").and_then(|v| v.as_str().map(String::from))
    }

    fn set_active_app_id(&self, id: &str) {
        self.store().set("active_app_id", id);
    }

    fn clear_active_app_id(&self) {
        self.store().delete("active_app_id");
    }

    pub async fn boot(&self) -> Result<(), String> {
        let url = self.core_url();
        if !self.client().is_available().await {
            return Err(format!("core daemon not reachable at {url}"));
        }
        info!("connected to core daemon at {url}");
        self.reconnect_or_cleanup().await;
        crate::browser::spawn_listener(url.clone());
        let _ = self.app_handle.emit("runtime-booted", ());
        Ok(())
    }

    async fn reconnect_or_cleanup(&self) {
        let app_id = match self.active_app_id() {
            Some(id) => id,
            None => return,
        };
        match self.client().worker_status(&app_id).await {
            Ok(status) if status == "running" => {
                info!(app_id, "reconnecting to running worker");
                self.subscribe_to_worker_logs(&app_id).await;
            }
            _ => {
                info!(app_id, "cleaning up orphaned worker");
                if let Err(e) = self.client().stop_worker(&app_id).await {
                    warn!(app_id, "orphan cleanup: {e}");
                }
                self.clear_active_app_id();
            }
        }
    }

    async fn subscribe_to_worker_logs(&self, app_id: &str) {
        if let Some(handle) = self.log_stream_abort.lock().await.take() {
            handle.abort();
        }

        let app_handle = self.app_handle.clone();
        let _ = app_handle.emit("run-started", ());

        let url = format!("{}/api/v1/apps/{app_id}/logs", self.core_url());
        let token = self.client().token();
        let abort_store = self.log_stream_abort.clone();

        let task = tokio::spawn(async move {
            let client = LOG_HTTP.clone();
            loop {
                let mut req = client.get(&url);
                if let Some(ref t) = token { req = req.bearer_auth(t); }
                match req.send().await {
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

    pub async fn publish_frontend(&self, project_path: &str) -> Result<String, String> {
        let project = Path::new(project_path);
        let manifest = read_manifest(project_path).await?;
        let app_id = manifest.app_id;
        let core_url = self.core_url();

        let pkg_manager = if project.join("bun.lock").exists() || project.join("bun.lockb").exists() {
            "bun"
        } else if project.join("pnpm-lock.yaml").exists() {
            "pnpm"
        } else {
            "npm"
        };

        let (shell, flag) = rootcx_platform::shell::shell_command();
        let output = tokio::process::Command::new(shell)
            .args([flag, &format!("{pkg_manager} run build -- --base=/apps/{app_id}/")])
            .current_dir(project)
            .env("VITE_ROOTCX_URL", &core_url)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("build failed: {e}"))?;

        if !output.status.success() {
            return Err(format!("frontend build failed:\n{}", String::from_utf8_lossy(&output.stderr)));
        }

        let dist_dir = project.join("dist");
        if !dist_dir.exists() {
            return Err("build did not produce a dist/ directory".into());
        }

        let archive = archive_dir(dist_dir, &[]).await?;
        info!(app_id = %app_id, size = archive.len(), "publishing frontend");
        self.client().deploy_frontend(&app_id, archive).await.map_err(|e| format!("publish failed: {e}"))?;

        Ok(format!("{core_url}/apps/{app_id}/"))
    }

    pub async fn deploy_backend(&self, project_path: &str) -> Result<String, String> {
        let project = Path::new(project_path);
        let manifest = read_manifest(project_path).await?;
        let app_id = manifest.app_id;

        let backend_dir = project.join("backend");
        if !backend_dir.exists() {
            return Err("no backend/ directory found in project".into());
        }

        let archive = archive_dir(backend_dir, DEPLOY_EXCLUDE).await?;
        info!(app_id = %app_id, size = archive.len(), "deploying backend");
        let result = self.client().deploy_app(&app_id, archive).await.map_err(|e| format!("deploy failed: {e}"))?;
        self.set_active_app_id(&app_id);
        Ok(result)
    }

    pub async fn deploy_and_watch(&self, project_path: &str) -> Result<String, String> {
        self.watchers.lock().await.clear();
        let msg = self.deploy_backend(project_path).await?;
        if let Some(app_id) = self.active_app_id() {
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
                            if let Some(app_id) = state.active_app_id() {
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
        if let Some(app_id) = self.active_app_id() {
            info!(app_id, "stopping deployed worker");
            if let Err(e) = self.client().stop_worker(&app_id).await {
                warn!(app_id, "failed to stop worker: {e}");
            }
        }
        self.clear_active_app_id();
        let _ = self.app_handle.emit("run-output", "\r\n[worker stopped]\r\n");
        self.watchers.lock().await.clear();
    }

    pub async fn shutdown(&self) {
        crate::browser::shutdown().await;
        if let Some(handle) = self.log_stream_abort.lock().await.take() {
            handle.abort();
        }
        self.watchers.lock().await.clear();
    }

    pub async fn status(&self) -> OsStatus {
        self.client().status().await.unwrap_or_else(|_| OsStatus::offline())
    }
}

async fn archive_dir(dir: PathBuf, exclude: &'static [&str]) -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(move || {
        let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(enc);
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))? {
            let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if exclude.contains(&name_str.as_ref()) { continue; }
            let path = entry.path();
            if path.is_file() {
                tar.append_path_with_name(&path, &*name_str).map_err(|e| format!("tar: {e}"))?;
            } else if path.is_dir() {
                tar.append_dir_all(&*name_str, &path).map_err(|e| format!("tar: {e}"))?;
            }
        }
        tar.into_inner().map_err(|e| format!("tar: {e}"))?.finish().map_err(|e| format!("gzip: {e}"))
    })
    .await
    .map_err(|e| format!("archive task: {e}"))?
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
