use std::path::{Path, PathBuf};

use tokio::process::Command;
use tracing::{info, warn};

use crate::PgError;

const PG_CONF_APPEND: &str = "listen_addresses = 'localhost'\n";

pub struct PostgresManager {
    bin_dir: PathBuf,
    lib_dir: Option<PathBuf>,
    data_dir: PathBuf,
    port: u16,
}

impl PostgresManager {
    pub fn new(bin_dir: PathBuf, data_dir: PathBuf, port: u16) -> Self {
        Self { bin_dir, lib_dir: None, data_dir, port }
    }

    pub fn with_lib_dir(mut self, lib_dir: PathBuf) -> Self {
        self.lib_dir = Some(lib_dir);
        self
    }

    pub fn port(&self) -> u16 { self.port }
    pub fn data_dir(&self) -> &Path { &self.data_dir }

    pub fn password(&self) -> String {
        std::fs::read_to_string(self.data_dir.join(".pg_password")).unwrap_or_default().trim().to_string()
    }

    pub fn connection_url(&self, db: &str) -> String {
        let pw = self.password();
        if pw.is_empty() {
            format!("postgres://localhost:{}/{db}", self.port)
        } else {
            format!("postgres://postgres:{pw}@localhost:{}/{db}", self.port)
        }
    }

    pub async fn init_db(&self) -> Result<(), PgError> {
        if self.data_dir.join("PG_VERSION").exists() {
            info!(data_dir = %self.data_dir.display(), "cluster already initialised, skipping initdb");
            return Ok(());
        }

        tokio::fs::create_dir_all(&self.data_dir)
            .await
            .map_err(|e| PgError::InitDb { data_dir: self.data_dir.clone(), source: e })?;

        info!(data_dir = %self.data_dir.display(), "running initdb");

        let password: String = (0..32).map(|_| {
            let idx = rand::random::<u8>() % 62;
            (match idx {
                0..=9 => b'0' + idx,
                10..=35 => b'a' + idx - 10,
                _ => b'A' + idx - 36,
            }) as char
        }).collect();

        let pwfile = self.data_dir.join(".pg_password");
        tokio::fs::write(&pwfile, &password).await
            .map_err(|e| PgError::InitDb { data_dir: self.data_dir.clone(), source: e })?;

        let output = self
            .pg_command("initdb")
            .args(["-D"]).arg(&self.data_dir)
            .args(["-E", "UTF8", "--locale=C", "--auth=scram-sha-256"])
            .arg("--pwfile").arg(&pwfile)
            .output().await
            .map_err(|e| PgError::InitDb { data_dir: self.data_dir.clone(), source: e })?;

        if !output.status.success() {
            return Err(PgError::InitDbFailed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        let conf_path = self.data_dir.join("postgresql.conf");
        if let Ok(mut conf) = tokio::fs::read_to_string(&conf_path).await {
            conf.push_str(PG_CONF_APPEND);
            let _ = tokio::fs::write(&conf_path, conf).await;
        }

        info!("initdb completed successfully");
        Ok(())
    }

    pub async fn start(&self) -> Result<(), PgError> {
        if self.is_running().await {
            info!(port = self.port, "postgres already running, skipping start");
            return Ok(());
        }

        info!(port = self.port, data_dir = %self.data_dir.display(), "starting postgres");

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            self.pg_command("pg_ctl")
                .args(["start", "-D"]).arg(&self.data_dir)
                .arg("-l").arg(self.data_dir.join("postmaster.log"))
                .arg("-o").arg(format!("-p {}", self.port))
                .arg("-w")
                .output()
        ).await
            .map_err(|_| PgError::Start { source: std::io::Error::new(std::io::ErrorKind::TimedOut, "pg_ctl start timed out (30s)") })?
            .map_err(|e| PgError::Start { source: e })?;

        if !output.status.success() {
            return Err(PgError::StartFailed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        info!(port = self.port, "postgres started");
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), PgError> {
        if !self.is_running().await {
            warn!("postgres is not running, nothing to stop");
            return Ok(());
        }

        info!("stopping postgres");

        let output = self
            .pg_command("pg_ctl")
            .args(["stop", "-D"]).arg(&self.data_dir)
            .args(["-m", "fast"])
            .output().await
            .map_err(|e| PgError::Stop { source: e })?;

        if !output.status.success() {
            return Err(PgError::StopFailed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        info!("postgres stopped");
        Ok(())
    }

    pub async fn is_running(&self) -> bool {
        self.pg_command("pg_ctl").args(["status", "-D"]).arg(&self.data_dir)
            .output().await.map(|o| o.status.success()).unwrap_or(false)
    }

    fn pg_command(&self, binary: &str) -> Command {
        let mut cmd = Command::new(rootcx_platform::bin::binary_path(&self.bin_dir, binary));
        cmd.env("PATH", rootcx_platform::env::prepend_path(&self.bin_dir));
        if let Some(lib_dir) = &self.lib_dir {
            if let Some(var) = rootcx_platform::env::dylib_path_var() {
                cmd.env(var, lib_dir);
            }
        }
        cmd
    }
}

pub fn data_base_dir() -> Result<PathBuf, PgError> {
    rootcx_platform::dirs::data_dir().map_err(|_| PgError::NoDataDir)
}
