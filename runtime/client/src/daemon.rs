use std::{path::PathBuf, time::{Duration, Instant}};
use crate::ClientError;

const HEALTH:         &str     = "http://localhost:9100/health";
const POLL:           Duration = Duration::from_millis(500);
const TIMEOUT_SPAWN:  Duration = Duration::from_secs(30);
const TIMEOUT_EXIST:  Duration = Duration::from_secs(15);

fn healthy() -> bool {
    reqwest::blocking::get(HEALTH).map(|r| r.status().is_success()).unwrap_or(false)
}

fn wait_healthy(timeout: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout { if healthy() { return true; } std::thread::sleep(POLL); }
    false
}

fn pid_path() -> Result<PathBuf, ClientError> {
    rootcx_platform::dirs::rootcx_home()
        .map(|h| h.join("runtime.pid"))
        .map_err(|e| ClientError::RuntimeStart(e.to_string()))
}

fn read_pid() -> Option<u32> {
    pid_path().ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse().ok())
}

fn err(s: impl Into<String>) -> ClientError { ClientError::RuntimeStart(s.into()) }

pub fn ensure_runtime() -> Result<(), ClientError> {
    if healthy() { return Ok(()); }

    if let Some(pid) = read_pid() {
        if rootcx_platform::process::process_alive(pid) {
            return if wait_healthy(TIMEOUT_EXIST) { Ok(()) }
                   else { Err(err(format!("pid {pid} alive but unresponsive"))) };
        }
    }

    let binary = rootcx_platform::bin::bundled_binary("rootcx-core")
        .ok_or_else(|| err("rootcx-core not found in app bundle"))?;

    let log_dir = rootcx_platform::dirs::rootcx_home()
        .map(|h| h.join("logs"))
        .map_err(|e| err(e.to_string()))?;
    std::fs::create_dir_all(&log_dir).map_err(|e| err(format!("log dir: {e}")))?;

    let log = std::fs::OpenOptions::new().create(true).append(true)
        .open(log_dir.join("runtime.log"))
        .map_err(|e| err(format!("log file: {e}")))?;

    spawn(&binary, log)?;
    if wait_healthy(TIMEOUT_SPAWN) { Ok(()) } else { Err(err("daemon unresponsive after spawn")) }
}

fn spawn(bin: &std::path::Path, log: std::fs::File) -> Result<(), ClientError> {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("--daemon")
       .stdout(log.try_clone().map_err(|e| err(format!("fd: {e}")))?)
       .stderr(log);
    // Detach so the daemon outlives Studio; CREATE_NO_WINDOW hides the console
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP:   u32 = 0x0000_0200;
        const CREATE_NO_WINDOW:           u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    cmd.spawn().map(|_| ()).map_err(|e| err(format!("spawn: {e}")))
}
