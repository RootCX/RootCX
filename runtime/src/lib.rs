mod api_error;
pub mod auth;
mod error;
pub mod extensions;
mod ipc;
mod jobs;
mod manifest;
mod routes;
mod schema;
mod scheduler;
mod secrets;
pub mod server;
mod worker;
mod worker_manager;

pub use error::RuntimeError;

use std::path::PathBuf;
use std::sync::Arc;

use auth::AuthConfig;
use extensions::rbac::PolicyCache;
use extensions::{builtin_extensions_with_cache, RuntimeExtension};
use rootcx_postgres_mgmt::PostgresManager;
use rootcx_shared_types::{ForgeStatus, OsStatus, PostgresStatus, RuntimeStatus, ServiceState};
use scheduler::SchedulerHandle;
use secrets::SecretManager;
use sqlx::PgPool;
use tracing::info;
use worker_manager::WorkerManager;

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Runtime {
    pg: PostgresManager,
    pool: Option<PgPool>,
    extensions: Vec<Box<dyn RuntimeExtension>>,
    auth_config: Arc<auth::AuthConfig>,
    rbac_cache: Arc<PolicyCache>,
    secret_manager: Option<Arc<SecretManager>>,
    worker_manager: Option<Arc<WorkerManager>>,
    scheduler: Option<SchedulerHandle>,
    data_dir: PathBuf,
}

impl Runtime {
    pub fn new(pg: PostgresManager, data_dir: PathBuf) -> Self {
        let auth_config = AuthConfig::load(&data_dir).expect("failed to load auth config");
        let rbac_cache = Arc::new(PolicyCache::default());
        let extensions = builtin_extensions_with_cache(
            Arc::clone(&auth_config),
            Arc::clone(&rbac_cache),
        );

        Self {
            pg,
            pool: None,
            extensions,
            auth_config,
            rbac_cache,
            secret_manager: None,
            worker_manager: None,
            scheduler: None,
            data_dir,
        }
    }

    pub async fn boot(&mut self, api_port: u16) -> Result<(), RuntimeError> {
        info!("runtime boot sequence starting");

        self.pg.init_db().await.map_err(RuntimeError::Postgres)?;
        self.pg.start().await.map_err(RuntimeError::Postgres)?;

        let db_url = format!("postgres://localhost:{}/postgres", self.pg.port());
        info!(url = %db_url, "connecting to postgres");

        let pool = PgPool::connect(&db_url).await.map_err(RuntimeError::Database)?;
        schema::bootstrap(&pool).await?;

        for ext in &self.extensions {
            ext.bootstrap(&pool).await?;
        }

        self.secret_manager = Some(Arc::new(SecretManager::new(&self.data_dir)?));

        let apps_dir = self.data_dir.join("apps");
        std::fs::create_dir_all(&apps_dir).map_err(|e| RuntimeError::Worker(format!("create apps dir: {e}")))?;
        let runtime_url = format!("http://127.0.0.1:{api_port}");
        let wm = Arc::new(WorkerManager::new(apps_dir, runtime_url));
        self.scheduler = Some(scheduler::spawn_scheduler(pool.clone(), Arc::clone(&wm)));
        self.worker_manager = Some(wm);

        self.pool = Some(pool);
        info!("runtime boot complete");
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        info!("runtime shutting down");
        if let Some(ref wm) = self.worker_manager { wm.stop_all().await; }
        self.worker_manager = None;
        if let Some(ref s) = self.scheduler { s.cancel.cancel(); }
        self.scheduler = None;

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

    pub fn auth_config(&self) -> &Arc<auth::AuthConfig> { &self.auth_config }

    pub fn rbac_cache(&self) -> &Arc<PolicyCache> { &self.rbac_cache }

    pub fn pool(&self) -> Option<&PgPool> { self.pool.as_ref() }

    pub fn extensions(&self) -> &[Box<dyn RuntimeExtension>] { &self.extensions }

    pub fn secret_manager(&self) -> Option<&Arc<SecretManager>> { self.secret_manager.as_ref() }

    pub fn worker_manager(&self) -> Option<&Arc<WorkerManager>> { self.worker_manager.as_ref() }

    pub fn scheduler_wake(&self) -> Option<&Arc<tokio::sync::Notify>> {
        self.scheduler.as_ref().map(|s| &s.wake)
    }

    pub fn data_dir(&self) -> &std::path::Path { &self.data_dir }
}
