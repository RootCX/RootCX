use std::sync::LazyLock;
use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::{info, warn};

pub const FORGE_PORT: u16 = 4096;
const HEALTH_URL: &str = "http://127.0.0.1:4096/global/health";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(HEALTH_TIMEOUT)
        .build()
        .expect("failed to build http client")
});

/// Manages the AI Forge sidecar process.
pub struct ForgeManager {
    child: Option<Child>,
}

impl ForgeManager {
    pub fn new() -> Self {
        Self { child: None }
    }

    /// Start the AI Forge sidecar in the given directory.
    ///
    /// If a process is already running, it is stopped first.
    /// If a stale process from a previous run is listening on the port,
    /// kill it first so the new instance starts cleanly.
    pub async fn start(
        &mut self,
        cwd: &std::path::Path,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ForgeError> {
        // Stop any running instance first.
        self.stop().await?;

        // Kill stale processes on the port before starting.
        if health_check().await {
            warn!("stale forge process detected on port {FORGE_PORT}, killing it");
            kill_listeners_on_port(FORGE_PORT).await;

            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if !health_check().await {
                    break;
                }
            }
        }

        info!(?cwd, ?config_path, "starting forge sidecar");

        let mut cmd = Command::new("opencode");
        cmd.args(["serve", "--port", "4096", "--hostname", "127.0.0.1"])
            .current_dir(cwd)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);

        if let Some(path) = config_path {
            cmd.env("OPENCODE_CONFIG", path);
        }

        let child = cmd
            .spawn()
            .map_err(|e| ForgeError::SpawnFailed(e.to_string()))?;

        self.child = Some(child);

        // Wait for health check (up to 15 seconds — forge may need to initialize)
        for i in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if health_check().await {
                info!("forge sidecar healthy after {}ms", (i + 1) * 500);
                return Ok(());
            }
        }

        warn!("forge sidecar started but health check timed out");
        Ok(())
    }

    /// Graceful stop: SIGTERM → wait up to 3s → SIGKILL.
    pub async fn stop(&mut self) -> Result<(), ForgeError> {
        if let Some(mut child) = self.child.take() {
            info!("stopping forge sidecar (graceful)");

            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // SAFETY: pid is a valid process ID obtained from Child::id().
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                for _ in 0..30 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    match child.try_wait() {
                        Ok(Some(_)) => {
                            info!("forge sidecar exited gracefully");
                            return Ok(());
                        }
                        Ok(None) => continue,
                        Err(_) => break,
                    }
                }
                warn!("forge sidecar did not exit after SIGTERM, sending SIGKILL");
            }

            let _ = child.kill().await;
            let _ = child.wait().await;
            info!("forge sidecar stopped");
        }
        Ok(())
    }

    /// Check if the sidecar is responding to health checks.
    pub async fn is_running(&self) -> bool {
        health_check().await
    }
}

/// HTTP health check against the Forge port.
async fn health_check() -> bool {
    HTTP.get(HEALTH_URL).send().await.is_ok()
}

/// Kill any processes listening on the given port (macOS/Linux).
async fn kill_listeners_on_port(port: u16) {
    let output = match tokio::process::Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            warn!("lsof failed: {e}");
            return;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pids: Vec<&str> = stdout.lines().collect();

    if pids.is_empty() {
        return;
    }

    info!(pids = ?pids, "killing stale forge processes on port {port}");

    for pid_str in &pids {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            #[cfg(unix)]
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }

    tokio::time::sleep(Duration::from_millis(500)).await;

    for pid_str in &pids {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            #[cfg(unix)]
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// Check if the `opencode` binary is available in PATH.
pub fn is_forge_available() -> bool {
    std::process::Command::new("opencode")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("failed to spawn forge sidecar: {0}")]
    SpawnFailed(String),
}
