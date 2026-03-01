use std::{path::PathBuf, time::{Duration, Instant}};
use crate::ClientError;

const PORT:          u16      = rootcx_platform::DEFAULT_API_PORT;
const POLL:          Duration = Duration::from_millis(500);
const TIMEOUT_SPAWN: Duration = Duration::from_secs(30);
const TIMEOUT_EXIST: Duration = Duration::from_secs(15);

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

fn sidecar_binary() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    for candidate in [
        dir.join(rootcx_platform::bin::binary_name(&format!("rootcx-core-{}", rootcx_platform::bin::TARGET_TRIPLE))),
        dir.join(rootcx_platform::bin::binary_name("rootcx-core")),
    ] {
        if candidate.is_file() { return Some(candidate); }
    }
    None
}

pub fn ensure_runtime() -> Result<RuntimeStatus, ClientError> {
    if healthy() { return Ok(RuntimeStatus::Ready); }

    // Existing process alive but slow to respond
    if let Some(pid) = read_pid() {
        if rootcx_platform::process::process_alive(pid) {
            return if wait_healthy(TIMEOUT_EXIST) { Ok(RuntimeStatus::Ready) }
                   else { Err(err(format!("pid {pid} alive but unresponsive"))) };
        }
    }

    // Already installed as service — just start it
    if let Some(bin) = installed_binary() {
        let log = open_log()?;
        spawn(&bin, log)?;
        return if wait_healthy(TIMEOUT_SPAWN) { Ok(RuntimeStatus::Ready) }
               else { Err(err("daemon unresponsive after spawn")) };
    }

    // Sidecar available — install as OS service, then wait
    if let Some(sidecar) = sidecar_binary() {
        let status = std::process::Command::new(&sidecar)
            .args(["install", "--service"])
            .status()
            .map_err(|e| err(format!("install: {e}")))?;
        if !status.success() {
            return Err(err("rootcx-core install --service failed"));
        }
        return if wait_healthy(TIMEOUT_SPAWN) { Ok(RuntimeStatus::Ready) }
               else { Err(err("daemon unresponsive after install")) };
    }

    Ok(RuntimeStatus::NotInstalled)
}

pub fn prompt_runtime_install() -> Result<(), ClientError> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"display dialog "RootCX Runtime is required but not installed.\nDownload and install it now?" buttons {"Cancel", "Install"} default button "Install" with title "RootCX""#;
        if !std::process::Command::new("osascript").args(["-e", script]).output()
            .is_ok_and(|o| o.status.success()) {
            return Err(err("RootCX Runtime installation cancelled"));
        }
        let url = runtime_download_url();
        if open::that(&url).is_err() {
            eprintln!("Download the runtime manually: {url}");
        }
        let deadline = Instant::now() + Duration::from_secs(300);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(2));
            if rootcx_platform::bin::runtime_installed() { return ensure_runtime().map(|_| ()); }
        }
        return Err(err("RootCX Runtime installation timed out"));
    }
    #[cfg(not(target_os = "macos"))]
    Err(err("RootCX Runtime is not installed. Please install it manually."))
}

pub fn deploy_bundled_backend(app_id: &str) {
    let Some(archive) = find_bundled_resource("backend.tar.gz") else { return };
    let Ok(data) = std::fs::read(&archive) else { return };
    let Ok(part) = reqwest::blocking::multipart::Part::bytes(data)
        .file_name("backend.tar.gz").mime_str("application/gzip") else { return };
    let _ = reqwest::blocking::Client::new()
        .post(format!("http://localhost:{PORT}/api/v1/apps/{app_id}/deploy"))
        .multipart(reqwest::blocking::multipart::Form::new().part("archive", part))
        .send();
}

fn runtime_download_url() -> String {
    let base = std::env::var("ROOTCX_RELEASE_URL")
        .unwrap_or_else(|_| "https://github.com/rootcx/rootcx/releases/latest/download".into());
    let triple = rootcx_platform::bin::TARGET_TRIPLE;
    #[cfg(target_os = "macos")]
    { return format!("{base}/RootCX-Runtime-{triple}.pkg"); }
    #[cfg(target_os = "windows")]
    { return format!("{base}/RootCX-Runtime-{triple}.exe"); }
    #[cfg(target_os = "linux")]
    { format!("{base}/RootCX-Runtime-{triple}.tar.gz") }
}

fn find_bundled_resource(name: &str) -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    #[cfg(target_os = "macos")]
    { let p = dir.parent()?.join("Resources/resources").join(name); if p.is_file() { return Some(p); } }
    let p = dir.join("resources").join(name);
    p.is_file().then_some(p)
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
        cmd.creation_flags(0x0000_0200 | 0x0800_0000); // CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
    }
    cmd.spawn().map(|_| ()).map_err(|e| err(format!("spawn: {e}")))
}
