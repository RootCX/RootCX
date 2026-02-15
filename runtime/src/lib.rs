mod api_error;
mod error;
mod manifest;
mod routes;
mod schema;
pub mod server;

pub use error::RuntimeError;

use rootcx_postgres_mgmt::PostgresManager;
use rootcx_shared_types::{ForgeStatus, RuntimeStatus, OsStatus, PostgresStatus, ServiceState};
use sqlx::PgPool;
use tracing::info;

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The RootCX Runtime — manages PostgreSQL lifecycle, schema bootstrap,
/// manifest engine, and the Collections CRUD API.
pub struct Runtime {
    pg: PostgresManager,
    pool: Option<PgPool>,
}

impl Runtime {
    pub fn new(pg: PostgresManager) -> Self {
        Self {
            pg,
            pool: None,
        }
    }

    /// Boot sequence: init cluster → start postgres → connect → bootstrap schema.
    pub async fn boot(&mut self) -> Result<(), RuntimeError> {
        info!("runtime boot sequence starting");

        self.pg.init_db().await.map_err(RuntimeError::Postgres)?;
        self.pg.start().await.map_err(RuntimeError::Postgres)?;

        let url = format!(
            "postgres://localhost:{}/postgres",
            self.pg.port()
        );
        info!(url = %url, "connecting to postgres");

        let pool = PgPool::connect(&url)
            .await
            .map_err(RuntimeError::Database)?;

        schema::bootstrap(&pool).await?;

        self.pool = Some(pool);

        info!("runtime boot complete");
        Ok(())
    }

    /// Graceful shutdown: close pool → stop postgres.
    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        info!("runtime shutting down");

        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }

        self.pg.stop().await.map_err(RuntimeError::Postgres)?;
        info!("runtime shutdown complete");
        Ok(())
    }

    /// Snapshot of the current OS status for the frontend.
    pub async fn status(&self) -> OsStatus {
        let pg_running = self.pg.is_running().await;

        OsStatus {
            runtime: RuntimeStatus {
                version: RUNTIME_VERSION.to_string(),
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
            forge: ForgeStatus {
                state: ServiceState::Offline,
                port: None,
            },
        }
    }

    /// Reference to the database pool (available after boot).
    pub fn pool(&self) -> Option<&PgPool> {
        self.pool.as_ref()
    }
}
