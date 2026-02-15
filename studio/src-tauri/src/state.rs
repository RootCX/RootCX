use std::path::PathBuf;
use std::sync::Arc;

use crate::forge::ForgeManager;
use rootcx_runtime_client::RuntimeClient;
use rootcx_shared_types::{ForgeStatus, OsStatus, ServiceState};
use tokio::sync::Mutex;
use tracing::{info, warn};

const PG_PORT: u16 = 5480;
const DAEMON_URL: &str = "http://localhost:9100";

#[derive(Clone)]
pub struct AppState {
    client: RuntimeClient,
    forge: Option<Arc<Mutex<ForgeManager>>>,
}

impl AppState {
    pub fn from_tauri(app: &tauri::App) -> Self {
        let forge = resolve_forge(app).map(|f| Arc::new(Mutex::new(f)));

        Self {
            client: RuntimeClient::new(DAEMON_URL),
            forge,
        }
    }

    pub async fn boot(&self) -> Result<(), String> {
        if !self.client.is_available().await {
            return Err(format!("runtime daemon not reachable at {DAEMON_URL}"));
        }
        info!("connected to runtime daemon at {DAEMON_URL}");

        if let Some(ref forge) = self.forge {
            if let Err(e) = forge.lock().await.start().await {
                warn!("forge sidecar start failed (non-fatal): {e}");
            }
        }

        Ok(())
    }

    pub async fn shutdown(&self) {
        if let Some(ref forge) = self.forge {
            if let Err(e) = forge.lock().await.stop().await {
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

        if let Some(ref forge) = self.forge {
            let running = forge.lock().await.is_running().await;
            status.forge = ForgeStatus {
                state: if running { ServiceState::Online } else { ServiceState::Offline },
                port: if running { Some(3100) } else { None },
            };
        }

        status
    }
}

fn resolve_forge(_app: &tauri::App) -> Option<ForgeManager> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ai_forge_dir = manifest_dir.parent()?.parent()?.join("forge");

    if !ai_forge_dir.join("src").join("ai_forge").exists() {
        info!("forge source not found, AI features disabled");
        return None;
    }

    let venv_python = ai_forge_dir.join(".venv").join("bin").join("python3");
    let python = if venv_python.exists() {
        venv_python.display().to_string()
    } else {
        "python3".to_string()
    };
    info!(dir = %ai_forge_dir.display(), python = %python, "forge source found");
    Some(ForgeManager::new_dev(&python, ai_forge_dir, PG_PORT))
}
