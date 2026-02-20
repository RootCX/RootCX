use std::path::{Path, PathBuf};

use tokio::process::Command;
use tracing::{info, warn};

use crate::PgError;

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

    pub async fn init_db(&self) -> Result<(), PgError> {
        if self.data_dir.join("PG_VERSION").exists() {
            info!(data_dir = %self.data_dir.display(), "cluster already initialised, skipping initdb");
            return Ok(());
        }

        tokio::fs::create_dir_all(&self.data_dir)
            .await
            .map_err(|e| PgError::InitDb { data_dir: self.data_dir.clone(), source: e })?;

        info!(data_dir = %self.data_dir.display(), "running initdb");

        let output = self
            .pg_command("initdb")
            .args(["-D"]).arg(&self.data_dir)
            .args(["-E", "UTF8", "--locale=C", "--auth=trust"])
            .output().await
            .map_err(|e| PgError::InitDb { data_dir: self.data_dir.clone(), source: e })?;

        if !output.status.success() {
            return Err(PgError::InitDbFailed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
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

        let output = self
            .pg_command("pg_ctl")
            .args(["start", "-D"]).arg(&self.data_dir)
            .arg("-l").arg(self.data_dir.join("postmaster.log"))
            .arg("-o").arg(format!("-p {}", self.port))
            .arg("-w")
            .output().await
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
