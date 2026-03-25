mod api_error;
pub mod auth;
mod error;
pub mod extensions;
mod ipc;
mod jobs;
mod manifest;
pub(crate) mod migrations;
pub mod mcp;
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
use rootcx_types::{ForgeStatus, OsStatus, PostgresStatus, RuntimeStatus, ServiceState};
use mcp::McpManager;
use scheduler::SchedulerHandle;
use secrets::SecretManager;
use sqlx::PgPool;
use tools::ToolRegistry;
use tracing::info;
use extensions::agents::approvals::PendingApprovals;
use worker_manager::WorkerManager;

const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Runtime {
    pg: PostgresManager,
    external_db: bool,
    pool: Option<PgPool>,
    extensions: Vec<Box<dyn RuntimeExtension>>,
    auth_config: Arc<auth::AuthConfig>,
    secret_manager: Option<Arc<SecretManager>>,
    worker_manager: Option<Arc<WorkerManager>>,
    tool_registry: Arc<ToolRegistry>,
    mcp_manager: Arc<McpManager>,
    pending_approvals: PendingApprovals,
    scheduler: Option<SchedulerHandle>,
    resources_dir: PathBuf,
    data_dir: PathBuf,
    bun_bin: PathBuf,
}

impl Runtime {
    pub fn new(pg: PostgresManager, data_dir: PathBuf, resources_dir: PathBuf, bun_bin: PathBuf) -> Self {
        let auth_config = AuthConfig::load(&data_dir).expect("failed to load auth config");

        let extensions = builtin_extensions(Arc::clone(&auth_config));

        let tool_registry = Arc::new(ToolRegistry::default());
        tool_registry.register(tools::query_data::QueryDataTool);
        tool_registry.register(tools::mutate_data::MutateDataTool);
        tool_registry.register(tools::list_apps::ListAppsTool);
        tool_registry.register(tools::describe_app::DescribeAppTool);
        tool_registry.register(tools::list_integrations::ListIntegrationsTool);
        tool_registry.register(tools::invoke_agent::InvokeAgentTool);

        let mcp_manager = Arc::new(McpManager::new(Arc::clone(&tool_registry), bun_bin.clone()));

        Self {
            pg,
            external_db: std::env::var("DATABASE_URL").is_ok(),
            pool: None,
            extensions,
            auth_config,
            secret_manager: None,
            worker_manager: None,
            tool_registry,
            mcp_manager,
            pending_approvals: PendingApprovals::new(),
            scheduler: None,
            resources_dir,
            data_dir,
            bun_bin,
        }
    }

    pub async fn boot(&mut self, api_port: u16) -> Result<(), RuntimeError> {
        info!("runtime boot sequence starting");

        let db_url = if self.external_db {
            let url = std::env::var("DATABASE_URL").unwrap();
            info!("using external database");
            url
        } else {
            self.pg.init_db().await.map_err(RuntimeError::Postgres)?;
            self.pg.start().await.map_err(RuntimeError::Postgres)?;
            info!("connecting to embedded postgres on port {}", self.pg.port());
            self.pg.connection_url("postgres")
        };

        let pool = PgPool::connect(&db_url).await.map_err(RuntimeError::Database)?;
        schema::bootstrap(&pool).await?;
        sqlx::migrate!("./migrations").run(&pool).await.map_err(|e| RuntimeError::Schema(e.into()))?;

        for ext in &self.extensions {
            ext.bootstrap(&pool).await?;
        }

        self.secret_manager = Some(Arc::new(SecretManager::new(&self.data_dir)?));

        let apps_dir = self.data_dir.join("apps");
        std::fs::create_dir_all(&apps_dir).map_err(|e| RuntimeError::Worker(format!("create apps dir: {e}")))?;
        let runtime_url = format!("http://127.0.0.1:{api_port}");
        let wm = Arc::new(WorkerManager::new(apps_dir, runtime_url, db_url, self.bun_bin.clone(), Arc::clone(&self.tool_registry), self.pending_approvals.clone()));
        self.scheduler = Some(scheduler::spawn_scheduler(pool.clone(), Arc::clone(&wm), Arc::clone(&self.auth_config)));
        self.worker_manager = Some(wm.clone());

        let secrets = self.secret_manager.as_ref().unwrap();
        wm.start_deployed_apps(&pool, secrets).await;

        self.tool_registry.sync_to_db(&pool).await;
        extensions::mcp::start_registered_servers(&pool, secrets, &self.mcp_manager).await;

        self.pool = Some(pool);
        info!("runtime boot complete");
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), RuntimeError> {
        info!("runtime shutting down");
        self.mcp_manager.stop_all().await;
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
        if !self.external_db {
            self.pg.stop().await.map_err(RuntimeError::Postgres)?;
        }
        info!("runtime shutdown complete");
        Ok(())
    }

    pub async fn status(&self) -> OsStatus {
        let pg_up = if self.external_db { self.pool.is_some() } else { self.pg.is_running().await };
        let online = ServiceState::Online;
        let offline = ServiceState::Offline;

        OsStatus {
            runtime: RuntimeStatus {
                version: RUNTIME_VERSION.to_string(),
                state: if self.pool.is_some() { online } else { offline },
            },
            postgres: PostgresStatus {
                state: if pg_up { online } else { offline },
                port: (!self.external_db && pg_up).then(|| self.pg.port()),
                data_dir: (!self.external_db).then(|| self.pg.data_dir().display().to_string()),
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

    pub fn mcp_manager(&self) -> &Arc<McpManager> {
        &self.mcp_manager
    }

    pub fn pending_approvals(&self) -> &PendingApprovals {
        &self.pending_approvals
    }

    pub fn scheduler_wake(&self) -> Option<&Arc<tokio::sync::Notify>> {
        self.scheduler.as_ref().map(|s| &s.wake)
    }

    pub fn bun_bin(&self) -> &std::path::Path {
        &self.bun_bin
    }

    pub fn resources_dir(&self) -> &std::path::Path {
        &self.resources_dir
    }

    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }
}
