use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::forge::{is_forge_available, ForgeManager, FORGE_PORT};
use notify_debouncer_mini::{new_debouncer, DebouncedEventKind};
use rootcx_runtime_client::RuntimeClient;
use rootcx_shared_types::{ForgeStatus, OsStatus, ServiceState};
use tokio::sync::Mutex;
use tracing::{info, warn};

const DAEMON_URL: &str = "http://localhost:9100";

// ── Embedded instruction files ──

const ROOTCX_RUNTIME_INSTRUCTIONS: &str =
    include_str!("../../../.agents/instructions/rootcx-runtime.md");

/// Default MCP servers always injected into the OpenCode config.
fn default_mcp_servers() -> serde_json::Value {
    serde_json::json!({
        "shadcn": {
            "type": "local",
            "command": ["npx", "shadcn@latest", "mcp"],
            "enabled": true
        }
    })
}

/// Enrich config JSON with default MCP servers and instruction paths.
fn enrich_config(config: &str) -> Result<String, String> {
    let mut obj: serde_json::Value =
        serde_json::from_str(config).map_err(|e| format!("invalid config JSON: {e}"))?;

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
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("failed to create config dir: {e}"))?;
    }
    let enriched = enrich_config(contents)?;
    tokio::fs::write(path, enriched.as_bytes())
        .await
        .map_err(|e| format!("failed to write config: {e}"))
}

#[derive(Clone)]
pub struct AppState {
    client: RuntimeClient,
    forge: Option<Arc<Mutex<ForgeManager>>>,
    /// Active file watchers (kept alive by holding the handle).
    #[allow(dead_code)]
    watchers: Arc<Mutex<Vec<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>>,
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "HOME not set".to_string())
}

pub fn config_dir() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".config/rootcx"))
}

pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config.json"))
}

pub fn instructions_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("instructions"))
}

pub fn sdk_runtime_dir() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/sdk")
        .canonicalize()
        .map_err(|e| format!("SDK not found: {e}"))
}

pub fn runtime_client_dir() -> Result<PathBuf, String> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../runtime/client")
        .canonicalize()
        .map_err(|e| format!("runtime client crate not found: {e}"))
}

async fn ensure_instructions() -> Result<(), String> {
    let dir = instructions_dir()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("failed to create instructions dir: {e}"))?;
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

impl AppState {
    pub fn from_tauri(_app: &tauri::App) -> Self {
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
        }
    }

    pub async fn boot(&self) -> Result<(), String> {
        if let Some(ref f) = self.forge {
            let cfg = ensure_config().await?;
            let cwd = home_dir()?;
            if let Err(e) = f.lock().await.start(&cwd, Some(cfg.as_path())).await {
                warn!("forge sidecar start failed (non-fatal): {e}");
            }
        }
        if !self.client.is_available().await {
            return Err(format!("runtime daemon not reachable at {DAEMON_URL}"));
        }
        info!("connected to runtime daemon at {DAEMON_URL}");
        Ok(())
    }

    pub async fn start_forge(&self, project_path: &str) -> Result<(), String> {
        if let Some(ref f) = self.forge {
            let cfg = ensure_config().await?;
            f.lock()
                .await
                .start(Path::new(project_path), Some(cfg.as_path()))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn save_forge_config(
        &self,
        contents: &str,
        project_path: Option<&str>,
    ) -> Result<(), String> {
        let path = config_path()?;
        write_config(&path, contents).await?;

        if let (Some(f), Some(pp)) = (&self.forge, project_path) {
            f.lock()
                .await
                .start(Path::new(pp), Some(&path))
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn sync_manifest(&self, project_path: &str) -> Result<(), String> {
        let contents = tokio::fs::read_to_string(Path::new(project_path).join("manifest.json"))
            .await
            .map_err(|e| format!("failed to read manifest: {e}"))?;
        let manifest: rootcx_shared_types::AppManifest =
            serde_json::from_str(&contents).map_err(|e| format!("invalid manifest: {e}"))?;
        if !manifest.data_contract.is_empty() {
            self.client.install_app(&manifest).await.map_err(|e| format!("failed to install app: {e}"))?;
        }
        Ok(())
    }

    /// Deploy `backend/` to the runtime. Use `deploy_and_watch` for hot reload.
    pub async fn deploy_backend(&self, project_path: &str) -> Result<String, String> {
        let project = Path::new(project_path);
        let manifest: serde_json::Value = serde_json::from_str(
            &tokio::fs::read_to_string(project.join("manifest.json"))
                .await
                .map_err(|e| format!("failed to read manifest: {e}"))?,
        )
        .map_err(|e| format!("invalid manifest: {e}"))?;
        let app_id = manifest["appId"]
            .as_str()
            .ok_or("manifest.json missing appId")?;

        let backend_dir = project.join("backend");
        if !backend_dir.exists() {
            return Err("no backend/ directory found in project".into());
        }

        let archive = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            let mut tar = tar::Builder::new(enc);
            tar.append_dir_all(".", &backend_dir)
                .map_err(|e| format!("tar failed: {e}"))?;
            tar.into_inner()
                .map_err(|e| format!("tar finalize failed: {e}"))?
                .finish()
                .map_err(|e| format!("gzip failed: {e}"))
        })
        .await
        .map_err(|e| format!("archive task failed: {e}"))??;

        info!(app_id, size = archive.len(), "deploying backend");
        self.client
            .deploy_app(app_id, archive)
            .await
            .map_err(|e| format!("deploy failed: {e}"))
    }

    /// Deploy backend + file watcher for hot reload.
    pub async fn deploy_and_watch(&self, project_path: &str) -> Result<String, String> {
        self.watchers.lock().await.clear();
        let msg = self.deploy_backend(project_path).await?;
        self.start_watcher(project_path)?;
        Ok(msg)
    }

    fn start_watcher(&self, project_path: &str) -> Result<(), String> {
        let backend_dir = Path::new(project_path).join("backend");
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer =
            new_debouncer(std::time::Duration::from_millis(500), tx)
                .map_err(|e| format!("watcher setup failed: {e}"))?;
        debouncer
            .watcher()
            .watch(&backend_dir, notify::RecursiveMode::Recursive)
            .map_err(|e| format!("watch failed: {e}"))?;

        info!(dir = %backend_dir.display(), "watching backend for hot reload");

        let state = self.clone();
        let project_path = project_path.to_string();
        let handle = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            while let Ok(Ok(events)) = rx.recv() {
                if events.iter().any(|e| e.kind == DebouncedEventKind::Any) {
                    info!("backend changed, redeploying");
                    match handle.block_on(state.deploy_backend(&project_path)) {
                        Ok(_) => info!("hot reload success"),
                        Err(e) => warn!("hot reload failed: {e}"),
                    }
                }
            }
        });

        let watchers = self.watchers.clone();
        tokio::spawn(async move { watchers.lock().await.push(debouncer) });
        Ok(())
    }

    pub async fn shutdown(&self) {
        if let Some(ref f) = self.forge {
            if let Err(e) = f.lock().await.stop().await {
                warn!("forge sidecar stop failed: {e}");
            }
        }
    }

    pub async fn status(&self) -> OsStatus {
        let mut status = self
            .client
            .status()
            .await
            .unwrap_or_else(|_| OsStatus::offline());

        if let Some(ref f) = self.forge {
            let running = f.lock().await.is_running().await;
            status.forge = ForgeStatus {
                state: if running { ServiceState::Online } else { ServiceState::Offline },
                port: if running { Some(FORGE_PORT) } else { None },
            };
        }
        status
    }
}
