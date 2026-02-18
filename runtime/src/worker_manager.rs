use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::future::join_all;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::secrets::SecretManager;
use crate::worker::{self, SupervisorHandle, WorkerConfig, WorkerStatus};
use crate::RuntimeError;

pub struct WorkerManager {
    workers: Arc<RwLock<HashMap<String, SupervisorHandle>>>,
    apps_dir: PathBuf,
    runtime_url: String,
    db_url: String,
}

impl WorkerManager {
    pub fn new(apps_dir: PathBuf, runtime_url: String, db_url: String) -> Self {
        Self { workers: Arc::new(RwLock::new(HashMap::new())), apps_dir, runtime_url, db_url }
    }

    async fn get_handle(&self, app_id: &str) -> Result<SupervisorHandle, RuntimeError> {
        self.workers.read().await.get(app_id).cloned()
            .ok_or_else(|| RuntimeError::Worker(format!("no worker for app '{app_id}'")))
    }

    pub async fn start_app(&self, pool: &PgPool, secrets: &SecretManager, app_id: &str) -> Result<(), RuntimeError> {
        if let Ok(h) = self.get_handle(app_id).await {
            if h.status().await? == WorkerStatus::Running {
                return Ok(());
            }
        }

        let app_dir = self.apps_dir.join(app_id);
        let entry_point = resolve_entry_point(&app_dir)?;
        let env: HashMap<String, String> = secrets.get_all_for_app(pool, app_id).await?.into_iter().collect();

        let config = WorkerConfig {
            app_id: app_id.to_string(),
            entry_point,
            working_dir: app_dir,
            env,
            runtime_url: self.runtime_url.clone(),
            db_url: self.db_url.clone(),
            pool: pool.clone(),
        };

        let handle = worker::spawn_supervisor(config);
        handle.start().await?;
        self.workers.write().await.insert(app_id.to_string(), handle);
        info!(app_id, "worker started");
        Ok(())
    }

    pub async fn stop_app(&self, app_id: &str) -> Result<(), RuntimeError> {
        // Clone the handle and drop the read lock BEFORE awaiting stop/write.
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

    pub async fn stop_all(&self) {
        let ids: Vec<String> = self.workers.read().await.keys().cloned().collect();
        for id in ids {
            if let Err(e) = self.stop_app(&id).await {
                error!(app_id = %id, "stop error: {e}");
            }
        }
    }

    pub async fn rpc(&self, app_id: &str, id: String, method: String, params: JsonValue) -> Result<JsonValue, RuntimeError> {
        self.get_handle(app_id).await?.rpc(id, method, params).await
    }

    pub async fn dispatch_job(&self, app_id: &str, job_id: String, payload: JsonValue) -> Result<(), RuntimeError> {
        self.get_handle(app_id).await?.dispatch_job(job_id, payload).await
    }

    pub async fn worker_status(&self, app_id: &str) -> Result<WorkerStatus, RuntimeError> {
        self.get_handle(app_id).await?.status().await
    }

    pub async fn all_statuses(&self) -> HashMap<String, WorkerStatus> {
        let handles: Vec<_> = self.workers.read().await
            .iter().map(|(id, h)| (id.clone(), h.clone())).collect();
        let futs = handles.into_iter().map(|(id, h)| async move {
            h.status().await.ok().map(|s| (id, s))
        });
        join_all(futs).await.into_iter().flatten().collect()
    }
}

fn resolve_entry_point(app_dir: &Path) -> Result<PathBuf, RuntimeError> {
    for name in ["index.ts", "index.js", "main.ts", "main.js", "src/index.ts", "src/index.js"] {
        let p = app_dir.join(name);
        if p.exists() { return Ok(p); }
    }
    Err(RuntimeError::Worker(format!("no entry point in {}", app_dir.display())))
}
