use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::forge::{is_forge_available, ForgeManager, FORGE_PORT};
use rootcx_runtime_client::RuntimeClient;
use rootcx_shared_types::{ForgeStatus, OsStatus, ServiceState};
use tokio::sync::Mutex;
use tracing::{info, warn};

const DAEMON_URL: &str = "http://localhost:9100";

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

fn config_path() -> Result<PathBuf, String> {
    Ok(home_dir()?.join(".config/rootcx/config.json"))
}

/// Returns the config path only if the file exists on disk.
fn existing_config() -> Option<PathBuf> {
    config_path().ok().filter(|p| p.exists())
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
            let cwd = home_dir()?;
            if let Err(e) = f.lock().await.start(&cwd, existing_config().as_deref()).await {
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
            f.lock()
                .await
                .start(Path::new(project_path), existing_config().as_deref())
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
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir)
                .await
                .map_err(|e| format!("failed to create config dir: {e}"))?;
        }
        tokio::fs::write(&path, contents.as_bytes())
            .await
            .map_err(|e| format!("failed to write config: {e}"))?;

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
