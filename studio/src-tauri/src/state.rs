use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::forge::{ForgeManager, is_forge_available};
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use rootcx_runtime::RuntimeClient;
use rootcx_shared_types::{ForgeStatus, OsStatus, ServiceState};
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tracing::{info, warn};

const DAEMON_URL: &str = "http://localhost:9100";

const ROOTCX_RUNTIME_INSTRUCTIONS: &str = include_str!("../../../.agents/instructions/rootcx-runtime.md");

fn default_mcp_servers() -> serde_json::Value {
    serde_json::json!({
        "shadcn": {
            "type": "local",
            "command": ["npx", "shadcn@latest", "mcp"],
            "enabled": true
        }
    })
}

fn enrich_config(config: &str) -> Result<String, String> {
    let mut obj: serde_json::Value = serde_json::from_str(config).map_err(|e| format!("invalid config JSON: {e}"))?;

    let defaults = default_mcp_servers();
    if let Some(existing) = obj.get_mut("mcp").and_then(|v| v.as_object_mut()) {
        for (key, value) in defaults.as_object().unwrap() {
            existing.entry(key.clone()).or_insert_with(|| value.clone());
        }
    } else {
        obj["mcp"] = defaults;
    }

    if let Ok(dir) = instructions_dir() {
        obj["instructions"] = serde_json::json!([dir.join("*.md").to_string_lossy().as_ref()]);
    }

    serde_json::to_string_pretty(&obj).map_err(|e| format!("failed to serialize config: {e}"))
}

async fn write_config(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(|e| format!("failed to create config dir: {e}"))?;
    }
    let enriched = enrich_config(contents)?;
    tokio::fs::write(path, enriched.as_bytes()).await.map_err(|e| format!("failed to write config: {e}"))
}

#[derive(Clone)]
pub struct AppState {
    client: RuntimeClient,
    forge: Option<Arc<Mutex<ForgeManager>>>,
    #[allow(dead_code)] // held alive for RAII
    watchers: Arc<Mutex<Vec<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>>,
    active_app_id: Arc<Mutex<Option<String>>>,
    app_handle: AppHandle,
    log_stream_abort: Arc<Mutex<Option<tokio::task::AbortHandle>>>,
}

fn home_dir() -> Result<PathBuf, String> { rootcx_platform::dirs::home_dir().map_err(|e| e.to_string()) }
pub fn config_dir() -> Result<PathBuf, String> { rootcx_platform::dirs::config_dir().map_err(|e| e.to_string()) }

pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config.json"))
}

pub fn instructions_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("instructions"))
}

fn session_file() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("studio-session.json"))
}

async fn ensure_instructions() -> Result<(), String> {
    let dir = instructions_dir()?;
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("failed to create instructions dir: {e}"))?;
    tokio::fs::write(dir.join("rootcx-runtime.md"), ROOTCX_RUNTIME_INSTRUCTIONS.as_bytes())
        .await
        .map_err(|e| format!("failed to write instruction file: {e}"))?;
    info!("installed instructions to {}", dir.display());
    Ok(())
}

async fn ensure_config() -> Result<PathBuf, String> {
    ensure_instructions().await?;
    let path = config_path()?;
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(s) => s,
        Err(_) => "{}".to_string(),
    };
    write_config(&path, &raw).await?;
    Ok(path)
}

async fn read_manifest(project_path: &str) -> Result<rootcx_shared_types::AppManifest, String> {
    let contents = tokio::fs::read_to_string(Path::new(project_path).join("manifest.json"))
        .await
        .map_err(|e| format!("failed to read manifest: {e}"))?;
    serde_json::from_str(&contents).map_err(|e| format!("invalid manifest: {e}"))
}

impl AppState {
    pub fn from_tauri(app: &tauri::App) -> Self {
        let forge = if is_forge_available() {
            info!("forge binary found, AI features enabled");
            Some(Arc::new(Mutex::new(ForgeManager::new())))
        } else {
            info!("forge binary not found in PATH, AI features disabled");
            None
        };
        Self {
            client: RuntimeClient::new(DAEMON_URL),
            forge,
            watchers: Arc::new(Mutex::new(Vec::new())),
            active_app_id: Arc::new(Mutex::new(None)),
            app_handle: app.handle().clone(),
            log_stream_abort: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn boot(&self) -> Result<(), String> {
        if let Some(ref f) = self.forge {
            let cfg = ensure_config().await?;
            let cwd = home_dir()?;
            let env = self.client.get_platform_env().await.unwrap_or_default();
            if let Err(e) = f.lock().await.start(&cwd, Some(cfg.as_path()), env).await {
                warn!("forge sidecar start failed (non-fatal): {e}");
            }
        }
        if !self.client.is_available().await {
            return Err(format!("core daemon not reachable at {DAEMON_URL}"));
        }
        info!("connected to core daemon at {DAEMON_URL}");

        self.reconnect_or_cleanup().await;

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
                // Check if the worker is still running — if so, reconnect to its log stream
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
            let client = reqwest::Client::new();
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

    pub async fn start_forge(&self, project_path: &str) -> Result<(), String> {
        if let Some(ref f) = self.forge {
            let cfg = ensure_config().await?;
            let env = self.client.get_platform_env().await.unwrap_or_default();
            f.lock().await.start(Path::new(project_path), Some(cfg.as_path()), env).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn save_forge_config(&self, contents: &str, project_path: Option<&str>) -> Result<(), String> {
        let path = config_path()?;
        write_config(&path, contents).await?;

        if let (Some(f), Some(pp)) = (&self.forge, project_path) {
            let env = self.client.get_platform_env().await.unwrap_or_default();
            f.lock().await.start(Path::new(pp), Some(&path), env).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn verify_schema(&self, project_path: &str) -> Result<rootcx_shared_types::SchemaVerification, String> {
        let manifest = read_manifest(project_path).await?;
        self.client.verify_schema(&manifest).await.map_err(|e| format!("verify failed: {e}"))
    }

    pub async fn sync_manifest(&self, project_path: &str) -> Result<(), String> {
        let manifest = read_manifest(project_path).await?;
        self.client.install_app(&manifest).await.map_err(|e| format!("failed to install app: {e}"))?;
        Ok(())
    }

    pub async fn deploy_backend(&self, project_path: &str) -> Result<String, String> {
        let project = Path::new(project_path);
        let manifest: serde_json::Value = serde_json::from_str(
            &tokio::fs::read_to_string(project.join("manifest.json"))
                .await
                .map_err(|e| format!("failed to read manifest: {e}"))?,
        )
        .map_err(|e| format!("invalid manifest: {e}"))?;
        let app_id = manifest["appId"].as_str().ok_or("manifest.json missing appId")?;

        // Agent projects have no backend/ dir — deploy the whole project instead
        let is_agent = manifest.get("agents").and_then(|a| a.as_object()).is_some_and(|a| !a.is_empty());
        let deploy_dir = if is_agent {
            project.to_path_buf()
        } else {
            let backend_dir = project.join("backend");
            if !backend_dir.exists() {
                return Err("no backend/ directory found in project".into());
            }
            backend_dir
        };

        let archive = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            if is_agent {
                // For agents: include manifest.json, agents/, package.json (skip node_modules, .git, src-tauri)
                for entry in ["manifest.json", "package.json", "agents"] {
                    let path = deploy_dir.join(entry);
                    if path.is_file() {
                        tar.append_path_with_name(&path, entry).map_err(|e| format!("tar failed: {e}"))?;
                    } else if path.is_dir() {
                        tar.append_dir_all(entry, &path).map_err(|e| format!("tar failed: {e}"))?;
                    }
                }
            } else {
                tar.append_dir_all(".", &deploy_dir).map_err(|e| format!("tar failed: {e}"))?;
            }
            tar.into_inner()
                .map_err(|e| format!("tar finalize failed: {e}"))?
                .finish()
                .map_err(|e| format!("gzip failed: {e}"))
        })
        .await
        .map_err(|e| format!("archive task failed: {e}"))??;

        info!(app_id, size = archive.len(), "deploying backend");
        let result = self.client.deploy_app(app_id, archive).await.map_err(|e| format!("deploy failed: {e}"))?;

        *self.active_app_id.lock().await = Some(app_id.to_string());
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
        // Watch agents/ dir for agent projects, backend/ for apps
        let watch_dir = if project.join("agents").exists() && !project.join("backend").exists() {
            project.join("agents")
        } else {
            project.join("backend")
        };
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

    pub async fn list_platform_secrets(&self) -> Result<Vec<String>, String> {
        self.client.list_platform_secrets().await.map_err(|e| format!("failed to list secrets: {e}"))
    }

    pub async fn set_platform_secret(&self, key: &str, value: &str) -> Result<(), String> {
        self.client.set_platform_secret(key, value).await.map_err(|e| format!("failed to set secret: {e}"))
    }

    pub async fn delete_platform_secret(&self, key: &str) -> Result<(), String> {
        self.client.delete_platform_secret(key).await.map_err(|e| format!("failed to delete secret: {e}"))
    }

    pub async fn shutdown(&self) {
        self.stop_deployed_worker().await;

        if let Some(ref f) = self.forge
            && let Err(e) = f.lock().await.stop().await {
                warn!("forge sidecar stop failed: {e}");
            }
    }

    pub async fn status(&self) -> OsStatus {
        let mut status = self.client.status().await.unwrap_or_else(|_| OsStatus::offline());

        if let Some(ref f) = self.forge {
            let fg = f.lock().await;
            let running = fg.is_running().await;
            status.forge = ForgeStatus {
                state: if running { ServiceState::Online } else { ServiceState::Offline },
                port: if running { Some(fg.port()) } else { None },
            };
        }
        status
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
