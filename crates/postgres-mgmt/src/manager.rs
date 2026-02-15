use std::path::{Path, PathBuf};

use tokio::process::Command;
use tracing::{info, warn};

use crate::PgError;

/// Manages the lifecycle of a local PostgreSQL instance.
///
/// Responsibilities:
/// - Locate PostgreSQL binaries (initdb, pg_ctl, postgres)
/// - Run `initdb` if the cluster doesn't exist yet
/// - Start / stop via `pg_ctl`
/// - Report whether the postmaster is running
///
/// The bundled PostgreSQL (Theseus portable build) uses standard relative-path
/// resolution: `<bindir>/../share` for timezone data, extensions, etc.
/// No config patching or env-var overrides are needed — just point `bin_dir`
/// at the real `bin/` inside the PostgreSQL tree.
pub struct PostgresManager {
    /// Directory containing PG binaries (initdb, pg_ctl, postgres).
    bin_dir: PathBuf,
    /// Bundled dylibs directory. Set DYLD_LIBRARY_PATH / LD_LIBRARY_PATH to this.
    lib_dir: Option<PathBuf>,
    /// Directory for the PG data cluster (PGDATA).
    data_dir: PathBuf,
    port: u16,
}

impl PostgresManager {
    // ── Construction ────────────────────────────────────────────────

    /// Create a manager with explicit paths (used by Tauri).
    pub fn new(bin_dir: PathBuf, data_dir: PathBuf, port: u16) -> Self {
        Self {
            bin_dir,
            lib_dir: None,
            data_dir,
            port,
        }
    }

    /// Set the bundled dylib directory. Enables DYLD_LIBRARY_PATH injection.
    pub fn with_lib_dir(mut self, lib_dir: PathBuf) -> Self {
        self.lib_dir = Some(lib_dir);
        self
    }

    // ── Public API ─────────────────────────────────────────────────

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Initialise the PostgreSQL cluster if the data directory is empty or absent.
    pub async fn init_db(&self) -> Result<(), PgError> {
        if self.data_dir.join("PG_VERSION").exists() {
            info!(data_dir = %self.data_dir.display(), "cluster already initialised, skipping initdb");
            return Ok(());
        }

        tokio::fs::create_dir_all(&self.data_dir)
            .await
            .map_err(|e| PgError::InitDb {
                data_dir: self.data_dir.clone(),
                source: e,
            })?;

        info!(data_dir = %self.data_dir.display(), "running initdb");

        let output = self
            .pg_command("initdb")
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-E")
            .arg("UTF8")
            .arg("--locale=C")
            .arg("--auth=trust")
            .output()
            .await
            .map_err(|e| PgError::InitDb {
                data_dir: self.data_dir.clone(),
                source: e,
            })?;

        if !output.status.success() {
            return Err(PgError::InitDbFailed {
                status: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }

        info!("initdb completed successfully");
        Ok(())
    }

    /// Start the PostgreSQL postmaster on the configured port.
    pub async fn start(&self) -> Result<(), PgError> {
        if self.is_running().await {
            info!(port = self.port, "postgres already running, skipping start");
            return Ok(());
        }

        info!(port = self.port, data_dir = %self.data_dir.display(), "starting postgres");

        let log_file = self.data_dir.join("postmaster.log");

        let output = self
            .pg_command("pg_ctl")
            .arg("start")
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-l")
            .arg(&log_file)
            .arg("-o")
            .arg(format!("-p {}", self.port))
            .arg("-w")
            .output()
            .await
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

    /// Stop the PostgreSQL postmaster gracefully.
    pub async fn stop(&self) -> Result<(), PgError> {
        if !self.is_running().await {
            warn!("postgres is not running, nothing to stop");
            return Ok(());
        }

        info!("stopping postgres");

        let output = self
            .pg_command("pg_ctl")
            .arg("stop")
            .arg("-D")
            .arg(&self.data_dir)
            .arg("-m")
            .arg("fast")
            .output()
            .await
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

    /// Check if the postmaster PID file exists and the process is alive.
    pub async fn is_running(&self) -> bool {
        let output = self
            .pg_command("pg_ctl")
            .arg("status")
            .arg("-D")
            .arg(&self.data_dir)
            .output()
            .await;

        match output {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
    }

    // ── Internals ──────────────────────────────────────────────────

    /// Build a `Command` for a PG binary with the correct environment.
    ///
    /// Only two env vars are injected:
    /// - `PATH`              : includes bin_dir so sub-processes find sibling binaries
    /// - `DYLD_LIBRARY_PATH` : bundled dylibs (macOS) / `LD_LIBRARY_PATH` (Linux)
    fn pg_command(&self, binary: &str) -> Command {
        let mut cmd = Command::new(self.bin_dir.join(binary));

        // PATH — so initdb finds postgres, pg_ctl finds postgres, etc.
        let mut path = self.bin_dir.display().to_string();
        if let Ok(existing) = std::env::var("PATH") {
            path = format!("{path}:{existing}");
        }
        cmd.env("PATH", &path);

        // Bundled dylib search path
        if let Some(lib_dir) = &self.lib_dir {
            #[cfg(target_os = "macos")]
            cmd.env("DYLD_LIBRARY_PATH", lib_dir);

            #[cfg(target_os = "linux")]
            cmd.env("LD_LIBRARY_PATH", lib_dir);
        }

        cmd
    }
}

/// Resolve the platform-specific RootCX data root.
pub fn data_base_dir() -> Result<PathBuf, PgError> {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("RootCX"));
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            return Ok(PathBuf::from(xdg).join("RootCX"));
        }
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("RootCX"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Ok(PathBuf::from(appdata).join("RootCX"));
        }
    }

    Err(PgError::NoDataDir)
}
