use std::path::Path;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::ProgressFn;

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_OUTPUT: usize = 30_000;
const PROGRESS_INTERVAL: usize = 500;

pub async fn bash(args: Value, cwd: &Path, on_progress: Option<ProgressFn>) -> Result<String, String> {
    let command = args["command"].as_str().ok_or("missing command")?;
    let timeout = args["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT);

    let mut child = Command::new("sh")
        .arg("-c").arg(command).current_dir(cwd)
        .stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("exec: {e}"))?;

    let (mut stdout_buf, mut last_emit) = (String::new(), 0usize);

    let stderr_handle = tokio::spawn({
        let stderr = child.stderr.take().unwrap();
        async move {
            let mut buf = String::new();
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !buf.is_empty() { buf.push('\n'); }
                buf.push_str(&line);
            }
            buf
        }
    });

    let result = tokio::time::timeout(std::time::Duration::from_secs(timeout), async {
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !stdout_buf.is_empty() { stdout_buf.push('\n'); }
            stdout_buf.push_str(&line);
            if let Some(ref cb) = on_progress {
                if stdout_buf.len() - last_emit >= PROGRESS_INTERVAL {
                    cb(&truncate(&stdout_buf, MAX_OUTPUT));
                    last_emit = stdout_buf.len();
                }
            }
            if stdout_buf.len() > MAX_OUTPUT { break; }
        }
        child.wait().await
    }).await;

    let stderr_buf = stderr_handle.await.unwrap_or_default();
    let code = match result {
        Ok(Ok(s)) => s.code().unwrap_or(-1),
        Ok(Err(e)) => return Err(format!("exec: {e}")),
        Err(_) => { let _ = child.kill().await; return Err(format!("timed out after {timeout}s")); }
    };

    let mut out = truncate(&stdout_buf, MAX_OUTPUT);
    if !stderr_buf.is_empty() {
        if !out.is_empty() { out.push('\n'); }
        out.push_str("stderr:\n");
        out.push_str(&truncate(&stderr_buf, MAX_OUTPUT));
    }
    if code != 0 { out.push_str(&format!("\nexit code: {code}")); }
    Ok(out)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { return s.to_string(); }
    let mut end = max;
    while !s.is_char_boundary(end) { end -= 1; }
    format!("{}...\n[truncated, {} total bytes]", &s[..end], s.len())
}
