pub(crate) mod queue;
pub(crate) mod routes;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;

pub struct BrowserExtension {
    queue: Arc<queue::BrowserQueue>,
}

impl BrowserExtension {
    pub fn new() -> Self {
        Self { queue: Arc::new(queue::BrowserQueue::new()) }
    }
}

#[async_trait]
impl RuntimeExtension for BrowserExtension {
    fn name(&self) -> &str {
        "browser"
    }

    async fn bootstrap(&self, _pool: &PgPool) -> Result<(), RuntimeError> {
        info!("browser extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        let q = Arc::clone(&self.queue);
        Some(
            Router::new()
                // Studio SSE + result
                .route("/api/v1/browser/commands", get(routes::command_stream))
                .route("/api/v1/browser/result/{cmd_id}", post(routes::submit_result))
                // Agent tool endpoints
                .route("/api/v1/browser/navigate", post(routes::navigate))
                .route("/api/v1/browser/snapshot", post(routes::snapshot))
                .route("/api/v1/browser/click", post(routes::click))
                .route("/api/v1/browser/type", post(routes::type_text))
                .route("/api/v1/browser/scroll", post(routes::scroll))
                .layer(axum::Extension(q)),
        )
    }
}
