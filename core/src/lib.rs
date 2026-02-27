mod api_error;
pub mod auth;
mod error;
pub mod extensions;
mod ipc;
mod jobs;
mod manifest;
mod routes;
mod scheduler;
mod schema;
mod schema_sync;
mod secrets;
pub mod server;
pub mod tools;
mod tool_executor;
mod worker;
mod worker_manager;

pub use error::RuntimeError;

use std::path::PathBuf;
use std::sync::Arc;

use auth::AuthConfig;
use extensions::{RuntimeExtension, builtin_extensions};
use rootcx_postgres_mgmt::PostgresManager;
use rootcx_shared_types::{ForgeStatus, OsStatus, PostgresStatus, RuntimeStatus, ServiceState};
use scheduler::SchedulerHandle;
use secrets::SecretManager;
use sqlx::PgPool;
use tools::ToolRegistry;
use tracing::info;
use worker_manager::WorkerManager;

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Runtime {
    pg: PostgresManager,
    pool: Option<PgPool>,
    extensions: Vec<Box<dyn RuntimeExtension>>,
    auth_config: Arc<auth::AuthConfig>,
    secret_manager: Option<Arc<SecretManager>>,
    worker_manager: Option<Arc<WorkerManager>>,
    tool_registry: Arc<ToolRegistry>,
    scheduler: Option<SchedulerHandle>,
    data_dir: PathBuf,
    bun_bin: PathBuf,
}

impl Runtime {
    pub fn new(pg: PostgresManager, data_dir: PathBuf, bun_bin: PathBuf) -> Self {
        Self::with_auth_mode(pg, data_dir, bun_bin, None)
    }

    pub fn with_auth_mode(
        pg: PostgresManager,
        data_dir: PathBuf,
        bun_bin: PathBuf,
        auth_required: Option<bool>,
    ) -> Self {
        let auth_config = AuthConfig::load(&data_dir, auth_required).expect("failed to load auth config");

        let browser_queue = Arc::new(extensions::browser::queue::BrowserQueue::new());
        let extensions = builtin_extensions(
            Arc::clone(&auth_config), Arc::clone(&browser_queue),
        );

        let mut tool_registry = ToolRegistry::default();
        tool_registry.register(tools::query_data::QueryDataTool);
        tool_registry.register(tools::mutate_data::MutateDataTool);
        tool_registry.register(tools::browser::BrowserTool::new(browser_queue));
        tool_registry.register(tools::list_apps::ListAppsTool);
        tool_registry.register(tools::describe_app::DescribeAppTool);

        Self {
            pg,
            pool: None,
            extensions,
            auth_config,
            secret_manager: None,
            worker_manager: None,
            tool_registry: Arc::new(tool_registry),
            scheduler: None,
            data_dir,
            bun_bin,
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
        let wm = Arc::new(WorkerManager::new(apps_dir, runtime_url, self.bun_bin.clone(), Arc::clone(&self.tool_registry)));
        self.scheduler = Some(scheduler::spawn_scheduler(pool.clone(), Arc::clone(&wm)));
        self.worker_manager = Some(wm);

        self.pool = Some(pool);
        info!("runtime boot complete");
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        info!("runtime shutting down");
        if let Some(ref wm) = self.worker_manager {
            wm.stop_all().await;
        }
        self.worker_manager = None;
        if let Some(ref s) = self.scheduler {
            s.cancel.cancel();
        }
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

    pub fn auth_config(&self) -> &Arc<auth::AuthConfig> {
        &self.auth_config
    }

    pub fn pool(&self) -> Option<&PgPool> {
        self.pool.as_ref()
    }

    pub fn extensions(&self) -> &[Box<dyn RuntimeExtension>] {
        &self.extensions
    }

    pub fn secret_manager(&self) -> Option<&Arc<SecretManager>> {
        self.secret_manager.as_ref()
    }

    pub fn worker_manager(&self) -> Option<&Arc<WorkerManager>> {
        self.worker_manager.as_ref()
    }

    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }

    pub fn scheduler_wake(&self) -> Option<&Arc<tokio::sync::Notify>> {
        self.scheduler.as_ref().map(|s| &s.wake)
    }

    pub fn bun_bin(&self) -> &std::path::Path {
        &self.bun_bin
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }
}
