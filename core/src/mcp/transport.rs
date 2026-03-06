use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use command_group::{AsyncCommandGroup, AsyncGroupChild};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{Mutex, oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};

use crate::RuntimeError;

const MAX_LINE: usize = 1_048_576;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<JsonValue>,
}

#[derive(Serialize)]
struct JsonRpcNotification {
    jsonrpc: &'static str,
    method: String,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    id: Option<u64>,
    result: Option<JsonValue>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    message: String,
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<JsonValue, String>>>>>;

fn mcp_err(msg: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Mcp(msg.to_string())
}

pub struct StdioTransport {
    child: Mutex<AsyncGroupChild>,
    stdin: Mutex<tokio::process::ChildStdin>,
    pending: PendingMap,
    next_id: AtomicU64,
    _reader: tokio::task::JoinHandle<()>,
}

impl StdioTransport {
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        working_dir: Option<&Path>,
    ) -> Result<Self, RuntimeError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(env);
        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        let mut child = cmd.group_spawn()
            .map_err(|e| mcp_err(format!("spawn '{command}': {e}")))?;

        let stdin = child.inner().stdin.take().ok_or_else(|| mcp_err("no stdin"))?;
        let stdout = child.inner().stdout.take().ok_or_else(|| mcp_err("no stdout"))?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        let reader = tokio::spawn(async move {
            let mut lines = FramedRead::new(
                tokio::io::BufReader::new(stdout),
                LinesCodec::new_with_max_length(MAX_LINE),
            );
            while let Some(Ok(line)) = lines.next().await {
                if line.trim().is_empty() { continue; }
                let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&line) else { continue };
                let Some(id) = resp.id else { continue };
                let result = match resp.error {
                    Some(e) => Err(e.message),
                    None => Ok(resp.result.unwrap_or(JsonValue::Null)),
                };
                if let Some(tx) = pending_clone.lock().await.remove(&id) {
                    let _ = tx.send(result);
                }
            }
        });

        Ok(Self {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            _reader: reader,
        })
    }

    pub async fn request(&self, method: &str, params: Option<JsonValue>) -> Result<JsonValue, RuntimeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let req = JsonRpcRequest { jsonrpc: "2.0", id, method: method.into(), params };
        self.write_json(&req).await?;

        match tokio::time::timeout(REQUEST_TIMEOUT, rx).await {
            Ok(Ok(Ok(val))) => Ok(val),
            Ok(Ok(Err(e))) => Err(RuntimeError::Mcp(e)),
            Ok(Err(_)) => Err(mcp_err("response channel dropped")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(mcp_err(format!("{method}: timeout (30s)")))
            }
        }
    }

    pub async fn notify(&self, method: &str) -> Result<(), RuntimeError> {
        self.write_json(&JsonRpcNotification { jsonrpc: "2.0", method: method.into() }).await
    }

    async fn write_json(&self, msg: &impl Serialize) -> Result<(), RuntimeError> {
        let mut line = serde_json::to_string(msg).map_err(mcp_err)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| mcp_err(format!("write: {e}")))?;
        stdin.flush().await.map_err(|e| mcp_err(format!("flush: {e}")))
    }

    pub async fn kill(&self) {
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self._reader.abort();
    }
}
