use tauri::{AppHandle, Emitter};
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;
use tracing::{info, warn};

#[derive(Default)]
pub struct RunnerState {
    kill_tx: Option<oneshot::Sender<()>>,
}

impl RunnerState {
    pub fn run(&mut self, command: &str, cwd: &str, app_handle: AppHandle) {
        self.stop();
        let _ = app_handle.emit("run-started", ());

        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        self.kill_tx = Some(kill_tx);

        let command = command.to_string();
        let cwd = cwd.to_string();

        tokio::spawn(async move {
            let mut child = match spawn_child(&command, &cwd) {
                Ok(c) => c,
                Err(e) => {
                    warn!("failed to spawn process: {e}");
                    let _ = app_handle.emit("run-output", format!("Failed to start: {e}\r\n"));
                    let _ = app_handle.emit("run-exited", Option::<i32>::None);
                    return;
                }
            };

            info!(command, cwd, "launched app process");

            if let Some(stdout) = child.stdout.take() {
                tokio::spawn(pipe_stream(stdout, app_handle.clone()));
            }
            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(pipe_stream(stderr, app_handle.clone()));
            }

            tokio::select! {
                status = child.wait() => {
                    let code = status.ok().and_then(|s| s.code());
                    info!(?code, "app process exited");
                    let _ = app_handle.emit("run-exited", code);
                }
                _ = kill_rx => {
                    info!("killing app process");
                    let _ = child.kill().await;
                }
            }
        });
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.kill_tx.take() {
            let _ = tx.send(());
        }
    }
}

async fn pipe_stream<R: AsyncReadExt + Unpin>(mut reader: R, handle: AppHandle) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let raw = String::from_utf8_lossy(&buf[..n]);
                let _ = handle.emit("run-output", raw.replace("\r\n", "\n").replace('\n', "\r\n"));
            }
        }
    }
}

fn spawn_child(command: &str, cwd: &str) -> Result<tokio::process::Child, std::io::Error> {
    let (shell, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
    tokio::process::Command::new(shell)
        .args([flag, command])
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
}
