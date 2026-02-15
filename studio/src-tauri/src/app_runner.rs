use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

const MAX_LOG_LINES: usize = 5000;

/// Manages a `cargo tauri dev` child process for a Forge-built app.
///
/// Captures stdout/stderr into a ring buffer that the frontend can poll.
pub struct AppRunner {
    project_path: PathBuf,
    child: Option<Child>,
    log_lines: Arc<Mutex<Vec<String>>>,
    log_offset: Arc<AtomicU64>,
}

impl AppRunner {
    pub fn new(project_path: PathBuf) -> Self {
        Self {
            project_path,
            child: None,
            log_lines: Arc::new(Mutex::new(Vec::new())),
            log_offset: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Start the app. Runs `npm install` first if `node_modules/` is missing,
    /// then spawns `cargo tauri dev` with piped stdout/stderr.
    pub async fn start(&mut self) -> Result<(), AppRunnerError> {
        if self.child.is_some() {
            return Err(AppRunnerError::AlreadyRunning);
        }

        let node_modules = self.project_path.join("node_modules");
        if !node_modules.exists() {
            self.append_log("[studio] node_modules not found, running npm install...").await;

            let output = Command::new("npm")
                .arg("install")
                .current_dir(&self.project_path)
                .output()
                .await
                .map_err(|e| AppRunnerError::SpawnFailed(format!("npm install: {e}")))?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stdout.lines().chain(stderr.lines()) {
                self.append_log(line).await;
            }

            if !output.status.success() {
                self.append_log("[studio] npm install failed").await;
                return Err(AppRunnerError::NpmInstallFailed);
            }
            self.append_log("[studio] npm install complete").await;
        }

        self.append_log("[studio] starting cargo tauri dev...").await;

        let mut child = Command::new("cargo")
            .args(["tauri", "dev"])
            .current_dir(&self.project_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| AppRunnerError::SpawnFailed(e.to_string()))?;

        // Spawn background tasks to read stdout and stderr into the log buffer
        let log_lines = Arc::clone(&self.log_lines);
        let log_offset = Arc::clone(&self.log_offset);

        if let Some(stdout) = child.stdout.take() {
            let lines = Arc::clone(&log_lines);
            let offset = Arc::clone(&log_offset);
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines_stream = reader.lines();
                while let Ok(Some(line)) = lines_stream.next_line().await {
                    push_log(&lines, &offset, line).await;
                }
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let lines = Arc::clone(&log_lines);
            let offset = Arc::clone(&log_offset);
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines_stream = reader.lines();
                while let Ok(Some(line)) = lines_stream.next_line().await {
                    push_log(&lines, &offset, format!("[stderr] {line}")).await;
                }
            });
        }

        info!(path = %self.project_path.display(), "app runner started");
        self.child = Some(child);
        Ok(())
    }

    /// Graceful stop: SIGTERM → 5s wait → SIGKILL.
    pub async fn stop(&mut self) -> Result<(), AppRunnerError> {
        if let Some(mut child) = self.child.take() {
            self.append_log("[studio] stopping app...").await;

            #[cfg(unix)]
            if let Some(pid) = child.id() {
                // SAFETY: pid is a valid process ID from Child::id().
                // Sending SIGTERM is safe; if the process already exited, kill()
                // returns an error which we ignore.
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                for _ in 0..50 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            self.append_log(&format!("[studio] app exited: {status}")).await;
                            return Ok(());
                        }
                        Ok(None) => continue,
                        Err(_) => break,
                    }
                }
                warn!("app did not exit after SIGTERM, sending SIGKILL");
            }

            let _ = child.kill().await;
            let _ = child.wait().await;
            self.append_log("[studio] app stopped (killed)").await;
            info!(path = %self.project_path.display(), "app runner stopped");
        }
        Ok(())
    }

    /// Return new log lines since `since` offset, plus the new offset.
    pub async fn read_logs(&self, since: u64) -> (Vec<String>, u64) {
        let current = self.log_offset.load(Ordering::Relaxed);
        if since >= current {
            return (vec![], current);
        }

        let lines = self.log_lines.lock().await;
        let total = lines.len() as u64;

        // The ring buffer may have wrapped — calculate how many lines exist
        // and how many are new since `since`.
        let available_from = if current > MAX_LOG_LINES as u64 {
            current - MAX_LOG_LINES as u64
        } else {
            0
        };

        let start = if since < available_from {
            0 // caller is behind the buffer — return everything we have
        } else {
            (since - available_from) as usize
        };

        let new_lines = lines[start..total as usize].to_vec();
        (new_lines, current)
    }

    async fn append_log(&self, line: &str) {
        push_log(&self.log_lines, &self.log_offset, line.to_string()).await;
    }
}

async fn push_log(lines: &Mutex<Vec<String>>, offset: &AtomicU64, line: String) {
    let mut buf = lines.lock().await;
    buf.push(line);
    if buf.len() > MAX_LOG_LINES {
        let drain = buf.len() - MAX_LOG_LINES;
        buf.drain(..drain);
    }
    offset.fetch_add(1, Ordering::Relaxed);
}

#[derive(Debug, thiserror::Error)]
pub enum AppRunnerError {
    #[error("app is already running")]
    AlreadyRunning,

    #[error("npm install failed")]
    NpmInstallFailed,

    #[error("failed to spawn process: {0}")]
    SpawnFailed(String),
}
