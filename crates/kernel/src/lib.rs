mod error;
pub mod manifest;
mod schema;

pub use error::KernelError;
pub use manifest::install_app;
pub use schema::{bootstrap, bootstrap_with_apps};

use std::path::PathBuf;

use rootcx_postgres_mgmt::PostgresManager;
use rootcx_shared_types::{KernelStatus, OsStatus, PostgresStatus, ServiceState};
use sqlx::PgPool;
use tracing::info;

const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The RootCX Kernel — supervisor of the local operating system.
///
/// Owns the PostgreSQL lifecycle and provides the system database pool.
/// Designed to later expose a Unix-socket "Syscalls" API to guest apps.
pub struct Kernel {
    pg: PostgresManager,
    pool: Option<PgPool>,
    /// Path to bundled app manifests (resources/apps/).
    apps_dir: Option<PathBuf>,
}

impl Kernel {
    pub fn new(pg: PostgresManager) -> Self {
        Self { pg, pool: None, apps_dir: None }
    }

    /// Set the directory containing bundled app manifests.
    pub fn with_apps_dir(mut self, apps_dir: PathBuf) -> Self {
        self.apps_dir = Some(apps_dir);
        self
    }

    /// Boot sequence: init cluster → start postgres → connect → bootstrap schema → install apps.
    pub async fn boot(&mut self) -> Result<(), KernelError> {
        info!("kernel boot sequence starting");

        // 1. Ensure the PG cluster exists.
        self.pg.init_db().await.map_err(KernelError::Postgres)?;

        // 2. Start the postmaster.
        self.pg.start().await.map_err(KernelError::Postgres)?;

        // 3. Connect with sqlx.
        let url = format!(
            "postgres://localhost:{}/postgres",
            self.pg.port()
        );
        info!(url = %url, "connecting to postgres");

        let pool = PgPool::connect(&url)
            .await
            .map_err(KernelError::Database)?;

        // 4. Bootstrap system schema + install all bundled apps.
        if let Some(ref apps_dir) = self.apps_dir {
            schema::bootstrap_with_apps(&pool, apps_dir).await?;
        } else {
            schema::bootstrap(&pool).await?;
        }

        self.pool = Some(pool);
        info!("kernel boot complete");
        Ok(())
    }

    /// Graceful shutdown: close pool → stop postgres.
    pub async fn shutdown(&mut self) -> Result<(), KernelError> {
        info!("kernel shutting down");

        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }

        self.pg.stop().await.map_err(KernelError::Postgres)?;
        info!("kernel shutdown complete");
        Ok(())
    }

    /// Snapshot of the current OS status for the frontend.
    pub async fn status(&self) -> OsStatus {
        let pg_running = self.pg.is_running().await;

        OsStatus {
            kernel: KernelStatus {
                version: KERNEL_VERSION.to_string(),
                state: if self.pool.is_some() {
                    ServiceState::Online
                } else {
                    ServiceState::Offline
                },
            },
            postgres: PostgresStatus {
                state: if pg_running {
                    ServiceState::Online
                } else {
                    ServiceState::Offline
                },
                port: if pg_running {
                    Some(self.pg.port())
                } else {
                    None
                },
                data_dir: Some(self.pg.data_dir().display().to_string()),
            },
        }
    }

    /// Reference to the database pool (available after boot).
    pub fn pool(&self) -> Option<&PgPool> {
        self.pool.as_ref()
    }
}
