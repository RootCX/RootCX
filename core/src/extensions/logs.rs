use std::convert::Infallible;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use futures::stream::Stream;
use sqlx::PgPool;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::routes::{self, SharedRuntime};

pub const LOG_CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub level: String,
    pub message: String,
}

pub fn emit_log(tx: &broadcast::Sender<LogEntry>, level: &str, message: impl Into<String>) {
    let _ = tx.send(LogEntry { level: level.to_string(), message: message.into() });
}

pub fn spawn_output_reader(
    reader: impl AsyncRead + Unpin + Send + 'static,
    level: &'static str,
    log_tx: broadcast::Sender<LogEntry>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            emit_log(&log_tx, level, &line);
        }
    })
}

pub struct LogsExtension;

#[async_trait]
impl RuntimeExtension for LogsExtension {
    fn name(&self) -> &str {
        "logs"
    }
    async fn bootstrap(&self, _pool: &PgPool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new().route("/api/v1/apps/{app_id}/logs", get(subscribe_worker_logs)))
    }
}

async fn subscribe_worker_logs(
    _identity: crate::auth::identity::Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let rx = routes::wm(&rt).subscribe_logs(&app_id).await?;

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(entry) => {
                let data = serde_json::to_string(&entry).unwrap_or_default();
                Some((Ok(Event::default().data(data)), rx))
            }
            Err(RecvError::Lagged(n)) => {
                let entry = LogEntry { level: "system".into(), message: format!("... {n} messages dropped ...") };
                let data = serde_json::to_string(&entry).unwrap_or_default();
                Some((Ok(Event::default().data(data)), rx))
            }
            Err(RecvError::Closed) => None,
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}
