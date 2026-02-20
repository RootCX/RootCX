use std::sync::LazyLock;
use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::{info, warn};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

static HTTP: LazyLock<reqwest::Client> =
    LazyLock::new(|| reqwest::Client::builder().timeout(HEALTH_TIMEOUT).build().expect("failed to build http client"));

pub struct ForgeManager {
    child: Option<Child>,
    port: u16,
}

impl ForgeManager {
    pub fn new() -> Self {
        let port = free_port();
        Self { child: None, port }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn start(
        &mut self,
        cwd: &std::path::Path,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ForgeError> {
        self.stop().await?;

        let port = self.port;
        let health_url = format!("http://127.0.0.1:{port}/global/health");

        if health_check_url(&health_url).await {
            warn!("stale forge process detected on port {port}, killing it");
            kill_listeners_on_port(port).await;

            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if !health_check_url(&health_url).await {
                    break;
                }
            }
        }

        info!(?cwd, ?config_path, port, "starting forge sidecar");

        let port_str = port.to_string();
        let mut cmd = Command::new("opencode");
        cmd.args(["serve", "--port", &port_str, "--hostname", "127.0.0.1"])
            .current_dir(cwd)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);

        if let Some(path) = config_path {
            cmd.env("OPENCODE_CONFIG", path);
        }

        let child = cmd.spawn().map_err(|e| ForgeError::SpawnFailed(e.to_string()))?;

        self.child = Some(child);

        for i in 0..30 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if health_check_url(&health_url).await {
                info!("forge sidecar healthy after {}ms", (i + 1) * 500);
                return Ok(());
            }
        }

        warn!("forge sidecar started but health check timed out");
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), ForgeError> {
        if let Some(mut child) = self.child.take() {
            info!("stopping forge sidecar (graceful)");

            #[cfg(unix)]
            if let Some(pid) = child.id() {
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

    pub async fn is_running(&self) -> bool {
        let url = format!("http://127.0.0.1:{}/global/health", self.port);
        health_check_url(&url).await
    }
}

async fn health_check_url(url: &str) -> bool {
    HTTP.get(url).send().await.is_ok()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(4096)
}

async fn kill_listeners_on_port(port: u16) {
    let output = match tokio::process::Command::new("lsof").args(["-ti", &format!(":{port}")]).output().await {
        Ok(o) => o,
        Err(e) => {
            warn!("lsof failed: {e}");
            return;
        }
    };

    let pids: Vec<i32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    if pids.is_empty() {
        return;
    }

    info!(?pids, "killing stale forge processes on port {port}");

    #[cfg(unix)]
    {
        for &pid in &pids {
            unsafe { libc::kill(pid, libc::SIGTERM); }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        for &pid in &pids {
            unsafe { libc::kill(pid, libc::SIGKILL); }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_uses_random_port() {
        let fm1 = ForgeManager::new();
        let fm2 = ForgeManager::new();
        assert_ne!(fm1.port(), 0);
        assert_ne!(fm2.port(), 0);
    }
}
