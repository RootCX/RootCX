use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};

use crate::{jobs, worker_manager::WorkerManager};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub fn spawn_scheduler(pool: PgPool, wm: Arc<WorkerManager>) -> Arc<Notify> {
    let wake = Arc::new(Notify::new());
    let w = Arc::clone(&wake);

    tokio::spawn(async move {
        info!("job scheduler started");
        loop {
            match jobs::claim_next(&pool).await {
                Ok(Some(job)) => {
                    let payload = job.payload.clone().unwrap_or(serde_json::json!({}));
                    if let Err(e) = wm.dispatch_job(&job.app_id, job.id.to_string(), payload).await {
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
            }
        }
    });

    wake
}
