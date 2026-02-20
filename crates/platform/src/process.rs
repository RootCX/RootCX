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
        let rounds = (timeout.as_millis() / poll.as_millis()).max(1) as u32;
        for _ in 0..rounds {
            tokio::time::sleep(poll).await;
            if !process_alive(pid) {
                info!(pid, "process exited after SIGTERM");
                return;
            }
        }
        warn!(pid, "SIGTERM timeout, sending SIGKILL");
        unsafe { libc::kill(pid as i32, libc::SIGKILL); }
    }
    #[cfg(windows)]
    {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output().await;
    }
}

pub async fn kill_listeners_on_port(port: u16) {
    let pids = find_listeners(port).await;
    if pids.is_empty() { return; }
    info!(?pids, port, "killing stale processes");
    for &pid in &pids {
        kill_gracefully(pid, Duration::from_millis(500)).await;
    }
}

async fn find_listeners(port: u16) -> Vec<u32> {
    #[cfg(unix)]
    {
        let Ok(output) = tokio::process::Command::new("lsof")
            .args(["-ti", &format!(":{port}")]).output().await
        else { return vec![]; };
        String::from_utf8_lossy(&output.stdout).lines().filter_map(|s| s.trim().parse().ok()).collect()
    }
    #[cfg(windows)]
    {
        let Ok(output) = tokio::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"]).output().await
        else { return vec![]; };
        let needle = format!(":{port}");
        String::from_utf8_lossy(&output.stdout).lines()
            .filter(|l| l.contains(&needle) && l.contains("LISTENING"))
            .filter_map(|l| l.split_whitespace().last()?.parse().ok())
            .collect::<std::collections::HashSet<u32>>().into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alive_check() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(4_000_000));
    }
}
