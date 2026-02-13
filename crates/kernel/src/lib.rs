mod error;
pub mod manifest;
mod schema;

pub use error::KernelError;
pub use manifest::{install_app, uninstall_app};
pub use schema::bootstrap;

use rootcx_postgres_mgmt::PostgresManager;
use rootcx_shared_types::{KernelStatus, OsStatus, PostgresStatus, ServiceState};
use sqlx::PgPool;
use tracing::info;

const KERNEL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The RootCX Kernel — supervisor of the local operating system.
///
/// Owns the PostgreSQL lifecycle and provides the system database pool.
pub struct Kernel {
    pg: PostgresManager,
    pool: Option<PgPool>,
}

impl Kernel {
    pub fn new(pg: PostgresManager) -> Self {
        Self { pg, pool: None }
    }

    /// Boot sequence: init cluster → start postgres → connect → bootstrap system schema.
    pub async fn boot(&mut self) -> Result<(), KernelError> {
        info!("kernel boot sequence starting");

        self.pg.init_db().await.map_err(KernelError::Postgres)?;
        self.pg.start().await.map_err(KernelError::Postgres)?;

        let url = format!(
            "postgres://localhost:{}/postgres",
            self.pg.port()
        );
        info!(url = %url, "connecting to postgres");

        let pool = PgPool::connect(&url)
            .await
            .map_err(KernelError::Database)?;

        schema::bootstrap(&pool).await?;

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
