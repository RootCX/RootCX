use anyhow::{Result, bail};
use std::time::Duration;

const COMPOSE_YAML: &str = r#"services:
  postgres:
    image: ghcr.io/rootcx/postgresql:16-pgmq
    user: root
    entrypoint: ["/pg-entrypoint.sh"]
    environment:
      POSTGRES_USER: rootcx
      POSTGRES_PASSWORD: rootcx
      POSTGRES_DB: rootcx
      PGDATA: /data/pgdata
    volumes:
      - pgdata:/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U rootcx -d rootcx"]
      interval: 2s
      timeout: 5s
      retries: 10
  core:
    image: ghcr.io/rootcx/core:latest
    depends_on:
      postgres:
        condition: service_healthy
    environment:
      DATABASE_URL: postgres://rootcx:rootcx@postgres:5432/rootcx
    ports:
      - "9100:9100"
    volumes:
      - data:/data
volumes:
  pgdata:
  data:
"#;

pub const LOCAL_URL: &str = "http://localhost:9100";

pub async fn check() -> bool {
    tokio::process::Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().await
        .map(|s| s.success())
        .unwrap_or(false)
}

pub async fn start_core() -> Result<()> {
    if is_healthy(LOCAL_URL).await { return Ok(()); }

    let dir = std::env::temp_dir().join("rootcx-compose");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("docker-compose.yml"), COMPOSE_YAML)?;

    let out = tokio::process::Command::new("docker")
        .args(["compose", "-f", &dir.join("docker-compose.yml").to_string_lossy(), "up", "-d", "--wait"])
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .output().await?;

    if !out.status.success() {
        bail!("docker compose up failed: {}", String::from_utf8_lossy(&out.stderr));
    }

    for _ in 0..90 {
        if is_healthy(LOCAL_URL).await { return Ok(()); }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    bail!("Core health check timed out (90s)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_yaml_exposes_core_on_9100() {
        assert!(COMPOSE_YAML.contains("\"9100:9100\""), "Core port mapping missing");
    }

    #[test]
    fn compose_yaml_uses_correct_pg_image() {
        assert!(COMPOSE_YAML.contains("ghcr.io/rootcx/postgresql:16-pgmq"));
    }

    #[test]
    fn compose_yaml_core_depends_on_healthy_postgres() {
        assert!(COMPOSE_YAML.contains("condition: service_healthy"));
    }

    #[test]
    fn local_url_matches_compose_port() {
        assert_eq!(LOCAL_URL, "http://localhost:9100");
    }
}

async fn is_healthy(base: &str) -> bool {
    reqwest::Client::new()
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(2))
        .send().await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}
