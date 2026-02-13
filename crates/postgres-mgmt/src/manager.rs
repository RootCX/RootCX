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

    /// Create a manager using platform-default paths (system PG installation).
    ///
    /// - **bin_dir** : auto-detected from common install locations.
    /// - **data_dir** :
    ///   - macOS  : `~/Library/Application Support/RootCX/data/pg`
    ///   - Linux  : `~/.local/share/RootCX/data/pg`
    ///   - Windows: `%APPDATA%/RootCX/data/pg`
    pub fn with_defaults(port: u16) -> Result<Self, PgError> {
        let bin_dir = discover_pg_bin_dir()?;
        let data_dir = data_base_dir()?.join("data").join("pg");

        info!(
            bin_dir = %bin_dir.display(),
            data_dir = %data_dir.display(),
            "resolved PostgreSQL paths"
        );

        Ok(Self {
            bin_dir,
            lib_dir: None,
            data_dir,
            port,
        })
    }

    // ── Public API ─────────────────────────────────────────────────

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
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

// ── Path Discovery (for system-installed PG) ───────────────────────

/// Search well-known locations for a directory containing `pg_ctl`.
fn discover_pg_bin_dir() -> Result<PathBuf, PgError> {
    // 1. Explicit override.
    if let Ok(dir) = std::env::var("ROOTCX_PG_BIN") {
        let p = PathBuf::from(&dir);
        if p.join("pg_ctl").exists() {
            return Ok(p);
        }
        warn!(dir = %dir, "ROOTCX_PG_BIN set but pg_ctl not found there, continuing search");
    }

    // 2/3. Homebrew paths (macOS ARM + Intel), multiple PG versions.
    let homebrew_prefixes = ["/opt/homebrew/opt", "/usr/local/opt"];
    let pg_versions = [
        "postgresql@17",
        "postgresql@16",
        "postgresql@15",
        "postgresql",
    ];

    for prefix in &homebrew_prefixes {
        for ver in &pg_versions {
            let candidate = PathBuf::from(prefix).join(ver).join("bin");
            if candidate.join("pg_ctl").exists() {
                return Ok(candidate);
            }
        }
    }

    // 4. Linux package manager paths.
    for ver in ["17", "16", "15"] {
        let candidate = PathBuf::from(format!("/usr/lib/postgresql/{ver}/bin"));
        if candidate.join("pg_ctl").exists() {
            return Ok(candidate);
        }
    }
    let usr_bin = PathBuf::from("/usr/bin");
    if usr_bin.join("pg_ctl").exists() {
        return Ok(usr_bin);
    }

    // 5. Windows.
    #[cfg(target_os = "windows")]
    {
        let program_files = std::env::var("ProgramFiles").unwrap_or_default();
        for ver in ["17", "16", "15"] {
            let candidate =
                PathBuf::from(&program_files).join(format!("PostgreSQL\\{ver}\\bin"));
            if candidate.join("pg_ctl.exe").exists() {
                return Ok(candidate);
            }
        }
    }

    // 6. Last resort: check PATH.
    if let Ok(output) = std::process::Command::new("which").arg("pg_ctl").output() {
        if output.status.success() {
            let path_str = String::from_utf8_lossy(&output.stdout);
            if let Some(parent) = PathBuf::from(path_str.trim()).parent() {
                return Ok(parent.to_path_buf());
            }
        }
    }

    Err(PgError::PgNotFound)
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
