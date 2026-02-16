use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::forge::{is_forge_available, ForgeManager, FORGE_PORT};
use rootcx_runtime_client::RuntimeClient;
use rootcx_shared_types::{ForgeStatus, OsStatus, ServiceState};
use tokio::sync::Mutex;
use tracing::{info, warn};

const DAEMON_URL: &str = "http://localhost:9100";

// ── Embedded skill files ──

const ROOTCX_RUNTIME_SKILL: &str =
    include_str!("../../../.agents/skills/rootcx-runtime/SKILL.md");

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

/// Merge default MCP servers into config JSON. User entries take precedence.
fn ensure_default_mcp(config: &str) -> Result<String, String> {
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

    // Ensure instructions point to the global skills directory (absolute path).
    // Replace if missing or if it still contains the old relative glob.
    let needs_update = match obj.get("instructions").and_then(|v| v.as_array()) {
        None => true,
        Some(arr) => arr.iter().any(|v| {
            v.as_str()
                .map(|s| s.starts_with(".agents/"))
                .unwrap_or(false)
        }),
    };
    if needs_update {
        if let Ok(dir) = skills_dir() {
            let glob = dir.join("*/SKILL.md").to_string_lossy().to_string();
            obj["instructions"] = serde_json::json!([glob]);
        }
    }

    serde_json::to_string_pretty(&obj).map_err(|e| format!("failed to serialize config: {e}"))
}

/// Write config JSON to disk, ensuring default MCP servers are present.
async fn write_config(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| format!("failed to create config dir: {e}"))?;
    }
    let enriched = ensure_default_mcp(contents)?;
    tokio::fs::write(path, enriched.as_bytes())
        .await
        .map_err(|e| format!("failed to write config: {e}"))
}

#[derive(Clone)]
pub struct AppState {
    client: RuntimeClient,
    forge: Option<Arc<Mutex<ForgeManager>>>,
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

pub fn skills_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("skills"))
}

/// Install embedded skill files to ~/.config/rootcx/skills/.
async fn ensure_skills() -> Result<(), String> {
    let dir = skills_dir()?.join("rootcx-runtime");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("failed to create skills dir: {e}"))?;
    tokio::fs::write(dir.join("SKILL.md"), ROOTCX_RUNTIME_SKILL.as_bytes())
        .await
        .map_err(|e| format!("failed to write skill file: {e}"))?;
    info!("installed skills to {}", dir.display());
    Ok(())
}

/// Ensure config file exists with default MCP servers, return its path.
async fn ensure_config() -> Result<PathBuf, String> {
    ensure_skills().await?;

    let path = config_path()?;
    let raw = if path.exists() {
        tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("failed to read config: {e}"))?
    } else {
        info!("no forge config found, creating default with MCP servers");
        "{}".to_string()
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
