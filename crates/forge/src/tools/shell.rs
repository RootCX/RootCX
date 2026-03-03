use std::path::Path;

use serde_json::Value;
use tokio::process::Command;

const DEFAULT_TIMEOUT: u64 = 120;
const MAX_OUTPUT: usize = 30_000;

pub async fn bash(args: Value, cwd: &Path) -> Result<String, String> {
    let command = args["command"].as_str().ok_or("missing command")?;
    let timeout_secs = args["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(cwd)
            .output(),
    )
    .await;

    let output = match result {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return Err(format!("exec: {e}")),
        Err(_) => return Err(format!("command timed out after {timeout_secs}s")),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let code = output.status.code().unwrap_or(-1);

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&truncate(&stdout, MAX_OUTPUT));
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("stderr:\n");
        result.push_str(&truncate(&stderr, MAX_OUTPUT));
    }

    if code != 0 {
        result.push_str(&format!("\nexit code: {code}"));
    }

    Ok(result)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) { end -= 1; }
        format!("{}...\n[truncated, {} total bytes]", &s[..end], s.len())
    }
}
