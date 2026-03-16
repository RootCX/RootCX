use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::auth::{AuthConfig, jwt};
use crate::ipc::RpcCaller;
use crate::{jobs, worker_manager::WorkerManager};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct SchedulerHandle {
    pub wake: Arc<Notify>,
    pub cancel: CancellationToken,
}

async fn resolve_caller(pool: &PgPool, auth: &AuthConfig, user_id: uuid::Uuid) -> Option<RpcCaller> {
    let (username,): (String,) = sqlx::query_as("SELECT username FROM rootcx_system.users WHERE id = $1")
        .bind(user_id).fetch_optional(pool).await.ok()??;
    let token = jwt::encode_access(auth, user_id, &username).ok()?;
    Some(RpcCaller { user_id: user_id.to_string(), username, auth_token: Some(token) })
}

pub fn spawn_scheduler(pool: PgPool, wm: Arc<WorkerManager>, auth_config: Arc<AuthConfig>) -> SchedulerHandle {
    let wake = Arc::new(Notify::new());
    let cancel = CancellationToken::new();
    let w = Arc::clone(&wake);
    let c = cancel.clone();

    tokio::spawn(async move {
        info!("job scheduler started");
        loop {
            if c.is_cancelled() { break; }
            match jobs::claim_next(&pool).await {
                Ok(Some(job)) => {
                    let payload = job.payload.clone().unwrap_or(serde_json::json!({}));
                    let caller = match job.user_id {
                        Some(uid) => resolve_caller(&pool, &auth_config, uid).await,
                        None => None,
                    };
                    if let Err(e) = wm.dispatch_job(&job.app_id, job.id.to_string(), payload, caller).await {
                        warn!(job_id = %job.id, "dispatch failed: {e}");
                        let _ = jobs::fail(&pool, job.id, &e.to_string()).await;
                    }
                    continue;
                }
                Ok(None) => {}
                Err(e) => error!("scheduler: {e}"),
            }
            tokio::select! {
                _ = tokio::time::sleep(POLL_INTERVAL) => {}
                _ = w.notified() => { debug!("scheduler woken"); }
                _ = c.cancelled() => { break; }
            }
        }
        info!("job scheduler stopped");
    });

    SchedulerHandle { wake, cancel }
}
