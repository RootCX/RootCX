use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use futures::future::join_all;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{error, info, warn};

use crate::RuntimeError;
use crate::extensions::agents::approvals::PendingApprovals;
use crate::extensions::logs::LogEntry;
use crate::ipc::{AgentBootConfig, AgentInvokePayload, RpcCaller};
use crate::secrets::SecretManager;
use crate::tools::{AgentDispatcher, ToolRegistry};
use crate::worker::{self, AgentEvent, SupervisorHandle, WorkerConfig, WorkerStatus};

const BACKEND_PRELUDE: &str = include_str!("backend_prelude.js");

pub struct WorkerManager {
    workers: Arc<RwLock<HashMap<String, SupervisorHandle>>>,
    dispatch: OnceLock<Arc<dyn AgentDispatcher>>,
    apps_dir: PathBuf,
    prelude_path: PathBuf,
    runtime_url: String,
    database_url: String,
    bun_bin: PathBuf,
    tool_registry: Arc<ToolRegistry>,
    pending_approvals: PendingApprovals,
}

impl WorkerManager {
    pub fn new(
        apps_dir: PathBuf, runtime_url: String, database_url: String, bun_bin: PathBuf,
        tool_registry: Arc<ToolRegistry>, pending_approvals: PendingApprovals,
    ) -> Self {
        let prelude_path = apps_dir.join(".prelude.js");
        std::fs::write(&prelude_path, BACKEND_PRELUDE).expect("write backend prelude");
        Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            dispatch: OnceLock::new(),
            apps_dir, prelude_path, runtime_url, database_url, bun_bin,
            tool_registry, pending_approvals,
        }
    }

    /// Must be called after wrapping in Arc to enable sub-agent dispatch.
    pub fn init_self_ref(self: &Arc<Self>) {
        let _ = self.dispatch.set(Arc::new(SubAgentDispatch { wm: Arc::clone(self) }));
    }

    async fn build_agent_boot(&self, pool: &PgPool, app_id: &str) -> Option<(AgentBootConfig, Option<rootcx_types::SupervisionConfig>)> {
        let config_json: serde_json::Value = sqlx::query_scalar(
            "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
        ).bind(app_id).fetch_optional(pool).await.ok()??;

        let agent_uid = crate::extensions::agents::agent_user_id(app_id);
        let (contract_res, perms_res) = tokio::join!(
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT COALESCE(manifest->'dataContract', '[]'::jsonb) FROM rootcx_system.apps WHERE id = $1",
            ).bind(app_id).fetch_optional(pool),
            crate::extensions::rbac::policy::resolve_permissions(pool, app_id, agent_uid),
        );

        let data_contract = contract_res.ok()?.unwrap_or_default();
        let (_, perms) = match perms_res {
            Ok(p) => p,
            Err(e) => { warn!(app_id, "agent boot: failed to resolve permissions: {e:?}"); return None; }
        };
        let tool_descriptors = self.tool_registry.descriptors_for_permissions(&perms, &data_contract);

        let max_turns = config_json.get("limits")
            .and_then(|l| l.get("maxTurns")).and_then(|v| v.as_u64()).unwrap_or(50) as u32;

        let supervision = config_json.get("supervision")
            .and_then(|v| serde_json::from_value::<rootcx_types::SupervisionConfig>(v.clone()).ok());

        Some((AgentBootConfig { tool_descriptors, max_turns }, supervision))
    }

    async fn get_handle(&self, app_id: &str) -> Result<SupervisorHandle, RuntimeError> {
        self.workers.read().await.get(app_id).cloned()
            .ok_or_else(|| RuntimeError::Worker(format!("no worker for app '{app_id}'")))
    }

    pub async fn start_app(&self, pool: &PgPool, secrets: &SecretManager, app_id: &str) -> Result<(), RuntimeError> {
        if let Ok(h) = self.get_handle(app_id).await
            && h.status().await? == WorkerStatus::Running {
                return Ok(());
            }

        let app_dir = self.apps_dir.join(app_id);
        let entry_point = resolve_entry_point(&app_dir)?;
        let credentials = secrets.get_env_for_app(pool, app_id).await?;

        let (agent_boot_config, supervision) = match self.build_agent_boot(pool, app_id).await {
            Some((boot, sup)) => (Some(boot), sup),
            None => (None, None),
        };

        let config = WorkerConfig {
            app_id: app_id.to_string(),
            entry_point,
            working_dir: app_dir,
            credentials,
            runtime_url: self.runtime_url.clone(),
            database_url: self.database_url.clone(),
            pool: pool.clone(),
            js_runtime: self.bun_bin.clone(),
            prelude_path: self.prelude_path.clone(),
            tool_registry: Arc::clone(&self.tool_registry),
            pending_approvals: self.pending_approvals.clone(),
            agent_dispatch: self.dispatch.get().cloned(),
            agent_boot_config,
            supervision,
        };

        let handle = worker::spawn_supervisor(config);
        handle.start().await?;
        self.workers.write().await.insert(app_id.to_string(), handle);
        info!(app_id, "worker started");
        Ok(())
    }

    pub async fn stop_app(&self, app_id: &str) -> Result<(), RuntimeError> {
        let handle = self.workers.read().await.get(app_id).cloned();
        if let Some(h) = handle {
            h.stop().await?;
            self.workers.write().await.remove(app_id);
            info!(app_id, "worker stopped");
        } else {
            warn!(app_id, "no worker to stop");
        }
        Ok(())
    }

    pub async fn start_deployed_apps(&self, pool: &PgPool, secrets: &SecretManager) {
        let Ok(entries) = std::fs::read_dir(&self.apps_dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || resolve_entry_point(&path).is_err() { continue; }
            let app_id = entry.file_name().to_string_lossy().to_string();

            if let Some(def) = crate::extensions::agents::config::load_agent_json(&path).await {
                if let Err(e) = crate::extensions::agents::register_agent(pool, &app_id, &def).await {
                    error!(app_id = %app_id, "re-register agent: {e}");
                }
            }
            if let Err(e) = self.start_app(pool, secrets, &app_id).await {
                error!(app_id = %app_id, "auto-start failed: {e}");
            }
        }
    }

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.workers.read().await.keys().cloned().collect();
        let futs = ids.iter().map(|id| async move { if let Err(e) = self.stop_app(id).await { error!(app_id = %id, "stop error: {e}"); } });
        join_all(futs).await;
    }

    pub async fn rpc(
        &self, app_id: &str, id: String, method: String, params: JsonValue, caller: Option<RpcCaller>,
    ) -> Result<JsonValue, RuntimeError> {
        self.get_handle(app_id).await?.rpc(id, method, params, caller).await
    }

    pub async fn agent_invoke(
        &self, app_id: &str, payload: AgentInvokePayload,
    ) -> Result<mpsc::Receiver<AgentEvent>, RuntimeError> {
        self.get_handle(app_id).await?.agent_invoke(payload).await
    }

    pub async fn dispatch_job(&self, app_id: &str, job_id: String, payload: JsonValue, caller: Option<RpcCaller>) -> Result<(), RuntimeError> {
        self.get_handle(app_id).await?.dispatch_job(job_id, payload, caller).await
    }

    pub async fn worker_status(&self, app_id: &str) -> Result<WorkerStatus, RuntimeError> {
        self.get_handle(app_id).await?.status().await
    }

    pub async fn subscribe_logs(&self, app_id: &str) -> Result<broadcast::Receiver<LogEntry>, RuntimeError> {
        Ok(self.get_handle(app_id).await?.subscribe())
    }

    pub async fn all_statuses(&self) -> HashMap<String, WorkerStatus> {
        let handles: Vec<_> = self.workers.read().await.iter().map(|(id, h)| (id.clone(), h.clone())).collect();
        let futs = handles.into_iter().map(|(id, h)| async move { h.status().await.ok().map(|s| (id, s)) });
        join_all(futs).await.into_iter().flatten().collect()
    }
}

// -- Sub-agent dispatch (implements AgentDispatcher for cross-worker invocation) --

struct SubAgentDispatch {
    wm: Arc<WorkerManager>,
}

#[async_trait]
impl AgentDispatcher for SubAgentDispatch {
    async fn dispatch(&self, _pool: &PgPool, caller: &str, target: &str, message: &str) -> Result<String, String> {
        if target == caller { return Err("cannot invoke self".into()); }

        let payload = AgentInvokePayload {
            invoke_id: uuid::Uuid::new_v4().to_string(),
            session_id: uuid::Uuid::new_v4().to_string(),
            message: message.to_string(),
            history: vec![],
            is_sub_invoke: true,
        };

        let mut rx = self.wm.agent_invoke(target, payload).await.map_err(|e| e.to_string())?;
        let mut response = String::new();
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Done { response: r, .. } => return Ok(r),
                AgentEvent::Error { error } => return Err(error),
                AgentEvent::Chunk { delta } => response.push_str(&delta),
                _ => {}
            }
        }
        if response.is_empty() { Err("no response from agent".into()) } else { Ok(response) }
    }
}

fn resolve_entry_point(app_dir: &Path) -> Result<PathBuf, RuntimeError> {
    for name in ["index.ts", "index.js", "main.ts", "main.js", "src/index.ts", "src/index.js"] {
        let p = app_dir.join(name);
        if p.exists() { return Ok(p); }
    }
    Err(RuntimeError::Worker(format!("no entry point in {}", app_dir.display())))
}
