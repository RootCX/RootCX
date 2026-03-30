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
    database_url: String,
    extensions: Vec<Box<dyn RuntimeExtension>>,
    auth_config: Arc<auth::AuthConfig>,
    tool_registry: Arc<ToolRegistry>,
    mcp_manager: Arc<McpManager>,
    pending_approvals: PendingApprovals,
    resources_dir: PathBuf,
    data_dir: PathBuf,
    bun_bin: PathBuf,
}

impl Runtime {
    pub fn new(database_url: String, data_dir: PathBuf, resources_dir: PathBuf, bun_bin: PathBuf) -> Self {
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
            database_url,
            extensions,
            auth_config,
            tool_registry,
            mcp_manager,
            pending_approvals: PendingApprovals::new(),
            resources_dir,
            data_dir,
            bun_bin,
        }
    }

    pub async fn boot(self, api_port: u16) -> Result<ReadyRuntime, RuntimeError> {
        info!("runtime boot sequence starting");

        let pool = PgPool::connect(&self.database_url).await.map_err(RuntimeError::Database)?;
        schema::bootstrap(&pool).await?;
        sqlx::migrate!("./migrations").run(&pool).await.map_err(|e| RuntimeError::Schema(e.into()))?;

        for ext in &self.extensions {
            ext.bootstrap(&pool).await?;
        }

        let secret_manager = Arc::new(SecretManager::new(&self.data_dir)?);

        let apps_dir = self.data_dir.join("apps");
        std::fs::create_dir_all(&apps_dir).map_err(|e| RuntimeError::Worker(format!("create apps dir: {e}")))?;
        let runtime_url = format!("http://127.0.0.1:{api_port}");
        let wm = Arc::new(WorkerManager::new(
            apps_dir, runtime_url, self.database_url.clone(), self.bun_bin.clone(),
            Arc::clone(&self.tool_registry), self.pending_approvals.clone(),
        ));
        wm.init_self_ref();
        let scheduler = scheduler::spawn_scheduler(pool.clone(), Arc::clone(&wm), Arc::clone(&self.auth_config));

        wm.start_deployed_apps(&pool, &secret_manager).await;

        self.tool_registry.sync_to_db(&pool).await;
        extensions::mcp::start_registered_servers(&pool, &secret_manager, &self.mcp_manager).await;

        info!("runtime boot complete");
        Ok(ReadyRuntime {
            pool,
            extensions: self.extensions,
            auth_config: self.auth_config,
            secret_manager,
            worker_manager: wm,
            tool_registry: self.tool_registry,
            mcp_manager: self.mcp_manager,
            pending_approvals: self.pending_approvals,
            scheduler,
            resources_dir: self.resources_dir,
            data_dir: self.data_dir,
            bun_bin: self.bun_bin,
        })
    }
}

/// Nothing mutates after boot — shared via Arc, no Mutex.
pub struct ReadyRuntime {
    pool: PgPool,
    extensions: Vec<Box<dyn RuntimeExtension>>,
    auth_config: Arc<auth::AuthConfig>,
    secret_manager: Arc<SecretManager>,
    worker_manager: Arc<WorkerManager>,
    tool_registry: Arc<ToolRegistry>,
    mcp_manager: Arc<McpManager>,
    pending_approvals: PendingApprovals,
    scheduler: SchedulerHandle,
    resources_dir: PathBuf,
    data_dir: PathBuf,
    bun_bin: PathBuf,
}

impl ReadyRuntime {
    pub async fn shutdown(&self) {
        info!("runtime shutting down");
        self.mcp_manager.stop_all().await;
        self.worker_manager.stop_all().await;
        self.scheduler.cancel.cancel();
        self.pool.close().await;
        info!("runtime shutdown complete");
    }

    pub fn status(&self) -> OsStatus {
        OsStatus {
            runtime: RuntimeStatus {
                version: RUNTIME_VERSION.to_string(),
                state: ServiceState::Online,
            },
            postgres: PostgresStatus {
                state: ServiceState::Online,
                port: None,
                data_dir: None,
            },
            forge: ForgeStatus { state: ServiceState::Offline, port: None },
        }
    }

    pub fn auth_config(&self) -> &Arc<auth::AuthConfig> { &self.auth_config }
    pub fn pool(&self) -> &PgPool { &self.pool }
    pub fn extensions(&self) -> &[Box<dyn RuntimeExtension>] { &self.extensions }
    pub fn secret_manager(&self) -> &Arc<SecretManager> { &self.secret_manager }
    pub fn worker_manager(&self) -> &Arc<WorkerManager> { &self.worker_manager }
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> { &self.tool_registry }
    pub fn mcp_manager(&self) -> &Arc<McpManager> { &self.mcp_manager }
    pub fn pending_approvals(&self) -> &PendingApprovals { &self.pending_approvals }
    pub fn scheduler_wake(&self) -> &Arc<tokio::sync::Notify> { &self.scheduler.wake }
    pub fn bun_bin(&self) -> &std::path::Path { &self.bun_bin }
    pub fn resources_dir(&self) -> &std::path::Path { &self.resources_dir }
    pub fn data_dir(&self) -> &std::path::Path { &self.data_dir }
}
