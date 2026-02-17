mod api_error;
mod error;
pub mod extensions;
mod manifest;
mod routes;
mod schema;
pub mod server;

pub use error::RuntimeError;

use extensions::{builtin_extensions, RuntimeExtension};
use rootcx_postgres_mgmt::PostgresManager;
use rootcx_shared_types::{ForgeStatus, OsStatus, PostgresStatus, RuntimeStatus, ServiceState};
use sqlx::PgPool;
use tracing::info;

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Runtime {
    pg: PostgresManager,
    pool: Option<PgPool>,
    extensions: Vec<Box<dyn RuntimeExtension>>,
}

impl Runtime {
    pub fn new(pg: PostgresManager) -> Self {
        Self { pg, pool: None, extensions: builtin_extensions() }
    }

    pub async fn boot(&mut self) -> Result<(), RuntimeError> {
        info!("runtime boot sequence starting");

        self.pg.init_db().await.map_err(RuntimeError::Postgres)?;
        self.pg.start().await.map_err(RuntimeError::Postgres)?;

        let url = format!("postgres://localhost:{}/postgres", self.pg.port());
        info!(url = %url, "connecting to postgres");

        let pool = PgPool::connect(&url).await.map_err(RuntimeError::Database)?;
        schema::bootstrap(&pool).await?;

        for ext in &self.extensions {
            ext.bootstrap(&pool).await?;
        }

        self.pool = Some(pool);
        info!("runtime boot complete");
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        info!("runtime shutting down");
        if let Some(pool) = self.pool.take() {
            pool.close().await;
        }
        self.pg.stop().await.map_err(RuntimeError::Postgres)?;
        info!("runtime shutdown complete");
        Ok(())
    }

    pub async fn status(&self) -> OsStatus {
        let pg_running = self.pg.is_running().await;
        let online = ServiceState::Online;
        let offline = ServiceState::Offline;

        OsStatus {
            runtime: RuntimeStatus {
                version: RUNTIME_VERSION.to_string(),
                state: if self.pool.is_some() { online } else { offline },
            },
            postgres: PostgresStatus {
                state: if pg_running { online } else { offline },
                port: pg_running.then(|| self.pg.port()),
                data_dir: Some(self.pg.data_dir().display().to_string()),
            },
            forge: ForgeStatus { state: offline, port: None },
        }
    }

    pub fn pool(&self) -> Option<&PgPool> {
        self.pool.as_ref()
    }

    pub fn extensions(&self) -> &[Box<dyn RuntimeExtension>] {
        &self.extensions
    }
}
