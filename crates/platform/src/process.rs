use std::time::Duration;
use tracing::{info, warn};

pub fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    { unsafe { libc::kill(pid as i32, 0) == 0 } }
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

pub async fn kill_gracefully(pid: u32, timeout: Duration) {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, libc::SIGTERM); }
        let poll = Duration::from_millis(100);
        for _ in 0..(timeout.as_millis() / poll.as_millis()).max(1) {
            tokio::time::sleep(poll).await;
            if !process_alive(pid) { info!(pid, "exited after SIGTERM"); return; }
        }
        warn!(pid, "SIGTERM timeout → SIGKILL");
        unsafe { libc::kill(pid as i32, libc::SIGKILL); }
    }
    #[cfg(windows)]
    {
        let _ = timeout; // Windows: force-kill is the only reliable option
        match tokio::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).status().await {
            Ok(s) if s.success() => info!(pid, "killed via taskkill"),
            Ok(s) => warn!(pid, code = ?s.code(), "taskkill non-zero"),
            Err(e) => warn!(pid, "taskkill: {e}"),
        }
    }
}

pub async fn kill_listeners_on_port(port: u16) {
    let pids = find_listeners(port).await;
    if pids.is_empty() { return; }
    info!(?pids, port, "killing stale listeners");
    for &pid in &pids { kill_gracefully(pid, Duration::from_millis(500)).await; }
}

// ── macOS ─────────────────────────────────────────────────────────────────────
// lsof ships on every macOS installation; fastest option available.
#[cfg(target_os = "macos")]
async fn find_listeners(port: u16) -> Vec<u32> {
    tokio::process::Command::new("lsof")
        .args(["-ti", &format!(":{port}")])
        .output().await
        .inspect_err(|e| warn!(port, "lsof: {e}"))
        .map(|o| String::from_utf8_lossy(&o.stdout).lines()
            .filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_default()
}

// ── Linux ─────────────────────────────────────────────────────────────────────
// Read /proc directly — zero external tool dependencies.
#[cfg(target_os = "linux")]
async fn find_listeners(port: u16) -> Vec<u32> {
    let hex = format!("{port:04X}");
    let inodes = tcp_inodes(&hex).await;
    if inodes.is_empty() { return vec![]; }
    pids_for_inodes(&inodes).await
}

// Parse /proc/net/tcp{,6}: columns are sl, local_addr (IP:PORT hex), …, state, …, inode
// State 0A = TCP_LISTEN; port is the last 4 hex chars of local_addr.
#[cfg(target_os = "linux")]
async fn tcp_inodes(hex_port: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(s) = tokio::fs::read_to_string(path).await else { continue };
        for line in s.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 10 || f[3] != "0A" { continue }
            if f[1].split(':').nth(1).is_some_and(|p| p.eq_ignore_ascii_case(hex_port)) {
                if let Ok(i) = f[9].parse() { out.push(i); }
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
async fn pids_for_inodes(inodes: &[u64]) -> Vec<u32> {
    let mut out = Vec::new();
    let Ok(mut dir) = tokio::fs::read_dir("/proc").await else { return out };
    while let Ok(Some(e)) = dir.next_entry().await {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(mut fds) = tokio::fs::read_dir(e.path().join("fd")).await else { continue };
        'fd: while let Ok(Some(fd)) = fds.next_entry().await {
            let Ok(lnk) = tokio::fs::read_link(fd.path()).await else { continue };
            if let Some(i) = lnk.to_string_lossy()
                .strip_prefix("socket:[").and_then(|s| s.strip_suffix(']'))
                .and_then(|s| s.parse::<u64>().ok())
            {
                if inodes.contains(&i) { out.push(pid); break 'fd; }
            }
        }
    }
    out
}

// ── Windows ───────────────────────────────────────────────────────────────────
#[cfg(windows)]
async fn find_listeners(port: u16) -> Vec<u32> {
    tokio::process::Command::new("netstat").args(["-ano", "-p", "TCP"])
        .output().await
        .inspect_err(|e| warn!(port, "netstat: {e}"))
        .map(|o| {
            let needle = format!(":{port}");
            String::from_utf8_lossy(&o.stdout).lines()
                .filter(|l| l.contains(&needle) && l.contains("LISTENING"))
                .filter_map(|l| l.split_whitespace().last()?.parse().ok())
                .collect::<std::collections::HashSet<u32>>().into_iter().collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn alive_check() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(4_000_000));
    }
}
