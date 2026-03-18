use std::{path::PathBuf, time::{Duration, Instant}};
use crate::ClientError;

const PORT:          u16      = rootcx_platform::DEFAULT_API_PORT;
const POLL:          Duration = Duration::from_millis(500);
const TIMEOUT_EXIST: Duration = Duration::from_secs(15);
const TIMEOUT_SPAWN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeStatus { Ready, NotInstalled }

fn healthy() -> bool {
    reqwest::blocking::get(format!("http://localhost:{PORT}/health")).is_ok_and(|r| r.status().is_success())
}

fn wait_healthy(timeout: Duration) -> bool {
    let t = Instant::now();
    while t.elapsed() < timeout { if healthy() { return true; } std::thread::sleep(POLL); }
    false
}

fn read_pid() -> Option<u32> {
    let p = rootcx_platform::dirs::rootcx_home().ok()?.join("runtime.pid");
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn err(s: impl Into<String>) -> ClientError { ClientError::RuntimeStart(s.into()) }

fn installed_binary() -> Option<PathBuf> {
    let p = rootcx_platform::dirs::rootcx_home().ok()?.join("bin").join(rootcx_platform::bin::binary_name("rootcx-core"));
    p.is_file().then_some(p)
}

pub fn ensure_runtime() -> Result<RuntimeStatus, ClientError> {
    if healthy() { return Ok(RuntimeStatus::Ready); }

    if let Some(pid) = read_pid() {
        if rootcx_platform::process::process_alive(pid) {
            return if wait_healthy(TIMEOUT_EXIST) { Ok(RuntimeStatus::Ready) }
                   else { Err(err(format!("pid {pid} alive but unresponsive"))) };
        }
    }

    if let Some(bin) = installed_binary() {
        let log = open_log()?;
        spawn(&bin, log)?;
        return if wait_healthy(TIMEOUT_SPAWN) { Ok(RuntimeStatus::Ready) }
               else { Err(err("daemon unresponsive after spawn")) };
    }

    Ok(RuntimeStatus::NotInstalled)
}

fn open_log() -> Result<std::fs::File, ClientError> {
    let log_dir = rootcx_platform::dirs::rootcx_home().map(|h| h.join("logs")).map_err(|e| err(e.to_string()))?;
    std::fs::create_dir_all(&log_dir).map_err(|e| err(format!("log dir: {e}")))?;
    std::fs::OpenOptions::new().create(true).append(true)
        .open(log_dir.join("runtime.log")).map_err(|e| err(format!("log file: {e}")))
}

fn spawn(bin: &std::path::Path, log: std::fs::File) -> Result<(), ClientError> {
    let mut cmd = std::process::Command::new(bin);
    cmd.arg("--daemon").stdout(log.try_clone().map_err(|e| err(format!("fd: {e}")))?).stderr(log);
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0200 | 0x0800_0000);
    }
    cmd.spawn().map(|_| ()).map_err(|e| err(format!("spawn: {e}")))
}
