use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::ClientError;

const HEALTH_URL: &str = "http://localhost:9100/health";
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const SPAWN_TIMEOUT: Duration = Duration::from_secs(30);
const EXISTING_TIMEOUT: Duration = Duration::from_secs(15);

fn rootcx_home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME not set")).join(".rootcx")
}

fn is_healthy() -> bool {
    reqwest::blocking::get(HEALTH_URL)
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn poll_health(timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if is_healthy() { return true; }
        std::thread::sleep(POLL_INTERVAL);
    }
    false
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool { false }

fn read_pid() -> Option<u32> {
    std::fs::read_to_string(rootcx_home().join("runtime.pid"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn runtime_binary() -> Option<PathBuf> {
    let bin = rootcx_home().join("bin/rootcx-runtime");
    bin.exists().then_some(bin)
}

fn err(msg: impl Into<String>) -> ClientError {
    ClientError::RuntimeStart(msg.into())
}

/// Ensure the RootCX Runtime daemon is running.
/// Checks health, then PID liveness, then spawns if needed.
pub fn ensure_runtime() -> Result<(), ClientError> {
    if is_healthy() { return Ok(()); }

    if let Some(pid) = read_pid() {
        if process_alive(pid) {
            return if poll_health(EXISTING_TIMEOUT) { Ok(()) }
            else { Err(err(format!("process {pid} alive but not healthy after {EXISTING_TIMEOUT:?}"))) };
        }
    }

    let binary = runtime_binary()
        .ok_or_else(|| err("~/.rootcx/bin/rootcx-runtime not found — run `rootcx-runtime install`"))?;

    let log_dir = rootcx_home().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log = std::fs::OpenOptions::new()
        .create(true).append(true)
        .open(log_dir.join("runtime.log"))
        .map_err(|e| err(format!("log file: {e}")))?;

    std::process::Command::new(binary)
        .arg("--daemon")
        .stdout(log.try_clone().map_err(|e| err(format!("clone fd: {e}")))?)
        .stderr(log)
        .spawn()
        .map_err(|e| err(format!("spawn: {e}")))?;

    if poll_health(SPAWN_TIMEOUT) { Ok(()) }
    else { Err(err(format!("not healthy within {SPAWN_TIMEOUT:?}"))) }
}
