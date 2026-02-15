use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::{info, warn};

const FORGE_PORT: u16 = 3100;
const HEALTH_URL: &str = "http://127.0.0.1:3100/health";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

/// Manages the AI Forge sidecar process.
pub struct ForgeManager {
    command: Vec<String>,
    working_dir: Option<PathBuf>,
    child: Option<Child>,
    pg_port: u16,
}

impl ForgeManager {
    /// Dev mode — spawn `python -m ai_forge` from the project directory.
    pub fn new_dev(python: &str, project_dir: PathBuf, pg_port: u16) -> Self {
        Self {
            command: vec![python.to_string(), "-m".to_string(), "ai_forge".to_string()],
            working_dir: Some(project_dir),
            child: None,
            pg_port,
        }
    }

    /// Start the forge sidecar.
    ///
    /// If a stale process from a previous run is listening on the forge port,
    /// kill it first so the new code is always loaded.
    pub async fn start(&mut self) -> Result<(), ForgeError> {
        // Always kill stale processes on the port before starting.
        // This ensures dev restarts always pick up new code.
        if self.child.is_none() && health_check().await {
            warn!("stale forge process detected on port {FORGE_PORT}, killing it");
            kill_listeners_on_port(FORGE_PORT).await;

            // Wait briefly for the port to be freed
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if !health_check().await {
                    break;
                }
            }
        }

        info!(command = ?self.command, "starting forge sidecar");

        let program = &self.command[0];
        let args = &self.command[1..];

        let mut cmd = Command::new(program);
        cmd.args(args)
            .env("FORGE_PG_PORT", self.pg_port.to_string())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        if let Some(ref cwd) = self.working_dir {
            cmd.current_dir(cwd);
        }

        let child = cmd
            .spawn()
            .map_err(|e| ForgeError::SpawnFailed(e.to_string()))?;

        self.child = Some(child);

        // Wait for health check (up to 10 seconds)
        for i in 0..20 {
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

            // Try SIGTERM first (gives uvicorn a chance to shut down)
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // SAFETY: pid is a valid process ID obtained from Child::id().
                // Sending SIGTERM to a valid PID is safe; if the process already
                // exited, kill() returns an error which we ignore.
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                // Wait up to 3 seconds for graceful exit
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

            // Fallback: SIGKILL
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

/// HTTP health check against the forge port.
async fn health_check() -> bool {
    let client = reqwest::Client::builder()
        .timeout(HEALTH_TIMEOUT)
        .build();

    match client {
        Ok(c) => c.get(HEALTH_URL).send().await.is_ok(),
        Err(_) => false,
    }
}

/// Kill any processes listening on the given port (macOS/Linux).
///
/// Uses `lsof` to find PIDs, then sends SIGTERM followed by SIGKILL.
async fn kill_listeners_on_port(port: u16) {
    // Use lsof to find PIDs listening on the port
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
            // SAFETY: pid parsed from lsof output — a valid numeric PID.
            // Sending signals to stale processes is safe; kernel ignores
            // signals to non-existent PIDs.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }

    // Wait a moment, then SIGKILL any survivors
    tokio::time::sleep(Duration::from_millis(500)).await;

    for pid_str in &pids {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            #[cfg(unix)]
            // SAFETY: same as above — valid PID, safe to signal.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("failed to spawn forge sidecar: {0}")]
    SpawnFailed(String),
}
