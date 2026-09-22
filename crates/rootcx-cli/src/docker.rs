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
      test: ["CMD-SHELL", "pg_isready -h 127.0.0.1 -U rootcx -d rootcx"]
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
      ROOTCX_PUBLIC_URL: http://localhost:9100
      ROOTCX_OIDC_ISSUER: ${ROOTCX_OIDC_ISSUER:?Set your OIDC issuer}
      ROOTCX_OIDC_CLIENT_ID: ${ROOTCX_OIDC_CLIENT_ID:?Set your OIDC client ID}
      ROOTCX_OIDC_CLIENT_SECRET: ${ROOTCX_OIDC_CLIENT_SECRET:?Set your OIDC client secret}
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

    for name in ["ROOTCX_OIDC_ISSUER", "ROOTCX_OIDC_CLIENT_ID", "ROOTCX_OIDC_CLIENT_SECRET"] {
        if std::env::var(name).map_or(true, |value| value.trim().is_empty()) {
            bail!("Set {name} before starting a self-hosted Core. Register http://localhost:9100/api/v1/auth/oidc/callback with your identity provider.");
        }
    }

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

async fn is_healthy(base: &str) -> bool {
    reqwest::Client::new()
        .get(format!("{base}/health"))
        .timeout(Duration::from_secs(2))
        .send().await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}
