use std::path::PathBuf;
use std::sync::Arc;

use rootcx_kernel::Kernel;
use rootcx_postgres_mgmt::{data_base_dir, PostgresManager};
use rootcx_shared_types::OsStatus;
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::info;

const PG_PORT: u16 = 5480;

/// Shared application state managed by Tauri.
///
/// Wraps the Kernel in an `Arc<Mutex<_>>` so it can be safely shared
/// across async Tauri command handlers.
#[derive(Clone)]
pub struct AppState {
    kernel: Arc<Mutex<Kernel>>,
}

impl AppState {
    /// Create from Tauri-bundled PostgreSQL paths.
    ///
    /// Bundle layout — a standard Theseus portable PostgreSQL tree:
    ///
    /// ```text
    /// resources/pg/<version>/
    /// +-- bin/       PG binaries (postgres, initdb, pg_ctl, …)
    /// +-- lib/       PG dylibs
    /// +-- share/     PG share data (timezones, extensions, …)
    /// ```
    ///
    /// PostgreSQL natively resolves `<bindir>/../share` and `<bindir>/../lib`,
    /// so timezone detection and extensions work without any env-var overrides
    /// or config patching.
    pub fn from_tauri(app: &tauri::App) -> Self {
        let (bin_dir, lib_dir, resource_root) = resolve_pg_paths(app);

        let data_dir = data_base_dir()
            .expect("failed to resolve data directory")
            .join("data")
            .join("pg");

        let apps_dir = resource_root.join("apps");

        info!(
            bin_dir  = %bin_dir.display(),
            lib_dir  = %lib_dir.display(),
            data_dir = %data_dir.display(),
            apps_dir = %apps_dir.display(),
            port     = PG_PORT,
            "initialising kernel with bundled PostgreSQL"
        );

        let pg = PostgresManager::new(bin_dir, data_dir, PG_PORT)
            .with_lib_dir(lib_dir);

        let kernel = Kernel::new(pg).with_apps_dir(apps_dir);

        Self {
            kernel: Arc::new(Mutex::new(kernel)),
        }
    }

    pub async fn boot(&self) -> Result<(), rootcx_kernel::KernelError> {
        self.kernel.lock().await.boot().await
    }

    pub async fn shutdown(&self) -> Result<(), rootcx_kernel::KernelError> {
        self.kernel.lock().await.shutdown().await
    }

    pub async fn status(&self) -> OsStatus {
        self.kernel.lock().await.status().await
    }

    /// Access the database pool (available after boot).
    pub async fn pool(&self) -> Option<PgPool> {
        self.kernel.lock().await.pool().cloned()
    }
}

/// Resolve PostgreSQL binary/lib paths and the resource root based on build mode.
///
/// Returns (bin_dir, lib_dir, resource_root).
fn resolve_pg_paths(app: &tauri::App) -> (PathBuf, PathBuf, PathBuf) {
    #[cfg(debug_assertions)]
    {
        let _ = app;
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let resource_root = manifest_dir.join("resources");
        let pg_root = find_pg_root(&resource_root.join("pg"))
            .expect("no PostgreSQL installation found in src-tauri/resources/pg/");
        let bin_dir = pg_root.join("bin");
        let lib_dir = pg_root.join("lib");
        return (bin_dir, lib_dir, resource_root);
    }

    #[cfg(not(debug_assertions))]
    {
        use tauri::Manager;
        let resource_dir = app
            .path()
            .resource_dir()
            .expect("failed to resolve resource directory");

        let resource_root = resource_dir.join("resources");
        let pg_root = find_pg_root(&resource_root.join("pg"))
            .expect("no PostgreSQL installation found in bundled resources");
        let bin_dir = pg_root.join("bin");
        let lib_dir = pg_root.join("lib");
        (bin_dir, lib_dir, resource_root)
    }
}

/// Find the PostgreSQL root directory inside a `resources/pg/` parent.
///
/// Looks for the first subdirectory containing `bin/postgres`.
fn find_pg_root(pg_dir: &std::path::Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(pg_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("bin").join("postgres").exists() {
            return Some(path);
        }
    }
    None
}
