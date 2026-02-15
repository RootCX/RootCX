use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::app_runner::AppRunner;
use crate::forge::ForgeManager;
use rootcx_runtime_client::RuntimeClient;
use rootcx_shared_types::{ForgeStatus, InstalledApp, AppManifest, OsStatus, ServiceState};
use tokio::sync::Mutex;
use tracing::{info, warn};

const PG_PORT: u16 = 5480;
const DAEMON_URL: &str = "http://localhost:9100";

/// Shared application state managed by Tauri.
///
/// Studio talks to the Runtime daemon over HTTP via `RuntimeClient`.
/// Forge sidecar and AppRunner are managed locally (Studio-only concerns).
#[derive(Clone)]
pub struct AppState {
    client: RuntimeClient,
    forge: Option<Arc<Mutex<ForgeManager>>>,
    running_apps: Arc<Mutex<HashMap<String, AppRunner>>>,
}

impl AppState {
    pub fn from_tauri(app: &tauri::App) -> Self {
        let forge = resolve_forge(app).map(|f| Arc::new(Mutex::new(f)));

        Self {
            client: RuntimeClient::new(DAEMON_URL),
            forge,
            running_apps: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn boot(&self) -> Result<(), String> {
        // Wait for the Runtime daemon to be available
        if !self.client.is_available().await {
            return Err(format!("runtime daemon not reachable at {DAEMON_URL}"));
        }
        info!("connected to runtime daemon at {DAEMON_URL}");

        // Start forge sidecar (non-fatal)
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

        // Overlay forge status (managed by Studio, not the daemon)
        if let Some(ref forge) = self.forge {
            let running = forge.lock().await.is_running().await;
            status.forge = ForgeStatus {
                state: if running { ServiceState::Online } else { ServiceState::Offline },
                port: if running { Some(3100) } else { None },
            };
        }

        status
    }

    pub async fn install_app_manifest(&self, manifest: &AppManifest) -> Result<String, String> {
        self.client.install_app(manifest).await.map_err(|e| e.to_string())
    }

    pub async fn list_installed_apps(&self) -> Result<Vec<InstalledApp>, String> {
        self.client.list_apps().await.map_err(|e| e.to_string())
    }

    // ── App Runner (Studio-only) ─────────────────────────

    pub async fn run_app(&self, project_path: String) -> Result<(), String> {
        {
            let apps = self.running_apps.lock().await;
            if apps.contains_key(&project_path) {
                return Err("app is already running".into());
            }
        }
        let mut runner = AppRunner::new(PathBuf::from(&project_path));
        runner.start().await.map_err(|e| e.to_string())?;
        self.running_apps.lock().await.insert(project_path, runner);
        Ok(())
    }

    pub async fn stop_app(&self, project_path: String) -> Result<(), String> {
        let mut apps = self.running_apps.lock().await;
        if let Some(mut runner) = apps.remove(&project_path) {
            runner.stop().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn app_logs(&self, project_path: &str, since: u64) -> (Vec<String>, u64) {
        let apps = self.running_apps.lock().await;
        match apps.get(project_path) {
            Some(runner) => runner.read_logs(since).await,
            None => (vec![], since),
        }
    }

    pub async fn running_app_paths(&self) -> Vec<String> {
        self.running_apps.lock().await.keys().cloned().collect()
    }

    pub async fn stop_all_apps(&self) {
        let mut apps = self.running_apps.lock().await;
        for (path, mut runner) in apps.drain() {
            if let Err(e) = runner.stop().await {
                warn!(path = %path, "failed to stop app: {e}");
            }
        }
    }
}

/// Resolve the AI Forge sidecar.
fn resolve_forge(app: &tauri::App) -> Option<ForgeManager> {
    #[cfg(debug_assertions)]
    {
        let _ = app;
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let ai_forge_dir = manifest_dir
            .parent()?
            .parent()?
            .join("forge");

        if ai_forge_dir.join("src").join("ai_forge").exists() {
            let venv_python = ai_forge_dir.join(".venv").join("bin").join("python3");
            let python = if venv_python.exists() {
                venv_python.display().to_string()
            } else {
                "python3".to_string()
            };
            info!(dir = %ai_forge_dir.display(), python = %python, "dev-mode forge source found");
            return Some(ForgeManager::new_dev(&python, ai_forge_dir, PG_PORT));
        }

        info!("forge source not found, AI features disabled");
        return None;
    }

    #[cfg(not(debug_assertions))]
    {
        let _ = app;
        info!("release-mode forge not yet supported");
        None
    }
}
