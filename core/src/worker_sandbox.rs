use std::process::Stdio;
use command_group::AsyncCommandGroup;
use tokio::process::Command;
use crate::{RuntimeError, worker::WorkerConfig};

pub(crate) const STORAGE_SOCKET: &str = "/tmp/rootcx-worker-storage.sock";
const SUPERVISOR: &str = "/opt/rootcx-worker-sandbox/supervisor.mjs";

pub(crate) fn enabled() -> Result<bool, RuntimeError> {
    match std::env::var("ROOTCX_WORKER_SANDBOX") {
        Err(std::env::VarError::NotPresent) => Ok(false),
        Ok(value) if value == "srt" && cfg!(target_os = "linux") => Ok(true),
        _ => Err(RuntimeError::Config("ROOTCX_WORKER_SANDBOX requires srt on Linux".into())),
    }
}

pub(crate) fn command(config: &WorkerConfig, env: Vec<(String, String)>) -> Result<Command, RuntimeError> {
    let mut command = Command::new(&config.js_runtime);
    command.env_clear();
    if enabled()? {
        if !std::path::Path::new(SUPERVISOR).is_file() {
            return Err(RuntimeError::Config("worker sandbox supervisor is unavailable".into()));
        }
        let spec = serde_json::json!({
            "root": config.working_dir,
            "prelude": config.prelude_path,
            "args": [config.js_runtime, "--preload", config.prelude_path, config.entry_point],
            "env": env.into_iter().collect::<std::collections::HashMap<_, _>>(),
            "storageSocket": STORAGE_SOCKET,
        });
        command.arg(SUPERVISOR).arg(spec.to_string())
            .current_dir("/opt/rootcx-worker-sandbox")
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", "/tmp");
    } else {
        command.arg("--preload").arg(&config.prelude_path).arg(&config.entry_point)
            .current_dir(&config.working_dir).envs(env);
    }
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    Ok(command)
}

#[cfg(unix)]
pub(crate) async fn storage_listener() -> Result<tokio::net::UnixListener, std::io::Error> {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    if let Ok(metadata) = std::fs::symlink_metadata(STORAGE_SOCKET) {
        if !metadata.file_type().is_socket() {
            return Err(std::io::Error::other("worker storage socket path is not a socket"));
        }
        std::fs::remove_file(STORAGE_SOCKET)?;
    }
    let listener = tokio::net::UnixListener::bind(STORAGE_SOCKET)?;
    std::fs::set_permissions(STORAGE_SOCKET, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub(crate) async fn install_dependencies(bun_bin: &std::path::Path, dir: &std::path::Path) -> Result<(), RuntimeError> {
    let mut args = vec![bun_bin.to_string_lossy().into_owned(), "install".into(), "--ignore-scripts".into()];
    if dir.join("bun.lock").is_file() || dir.join("bun.lockb").is_file() {
        args.push("--frozen-lockfile".into());
    }
    let spec = serde_json::json!({
        "root": dir, "args": args, "env": {}, "writeRoot": true, "timeoutMs": 120_000,
    });
    let mut child = Command::new(bun_bin).env_clear().arg(SUPERVISOR).arg(spec.to_string())
        .current_dir("/opt/rootcx-worker-sandbox").env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("HOME", "/tmp").kill_on_drop(true)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).group_spawn()
        .map_err(|e| RuntimeError::Worker(format!("isolated dependency installation: {e}")))?;
    let status = match tokio::time::timeout(std::time::Duration::from_secs(130), child.wait()).await {
        Ok(status) => status.map_err(|e| RuntimeError::Worker(format!("isolated dependency installation: {e}")))?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(RuntimeError::Worker("isolated dependency installation timed out".into()));
        }
    };
    if !status.success() {
        return Err(RuntimeError::Worker("isolated dependency installation failed".into()));
    }
    Ok(())
}
