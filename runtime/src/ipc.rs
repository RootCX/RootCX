use std::collections::HashMap;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::oneshot;
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::{error, warn};

const MAX_LINE_LENGTH: usize = 1_048_576;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutboundMessage {
    Discover { app_id: String, runtime_url: String, db_url: String },
    Rpc { id: String, method: String, params: JsonValue },
    Job { id: String, payload: JsonValue },
    Shutdown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboundMessage {
    Discover { #[serde(default)] capabilities: Vec<String> },
    RpcResponse { id: String, #[serde(default)] result: Option<JsonValue>, #[serde(default)] error: Option<String> },
    JobResult { id: String, #[serde(default)] result: Option<JsonValue>, #[serde(default)] error: Option<String> },
    Log { #[serde(default = "default_level")] level: String, message: String },
}

fn default_level() -> String { "info".into() }

pub struct IpcWriter { stdin: ChildStdin }

impl IpcWriter {
    pub fn new(stdin: ChildStdin) -> Self { Self { stdin } }

    pub async fn send(&mut self, msg: &OutboundMessage) -> Result<(), crate::RuntimeError> {
        let mut line = serde_json::to_string(msg)
            .map_err(|e| crate::RuntimeError::Ipc(e.to_string()))?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await
            .map_err(|e| crate::RuntimeError::Ipc(e.to_string()))?;
        self.stdin.flush().await
            .map_err(|e| crate::RuntimeError::Ipc(e.to_string()))
    }
}

pub struct IpcReader {
    lines: FramedRead<BufReader<ChildStdout>, LinesCodec>,
}

impl IpcReader {
    pub fn new(stdout: ChildStdout) -> Self {
        Self { lines: FramedRead::new(BufReader::new(stdout), LinesCodec::new_with_max_length(MAX_LINE_LENGTH)) }
    }

    pub async fn recv(&mut self) -> Option<InboundMessage> {
        loop {
            match self.lines.next().await {
                Some(Ok(line)) if line.trim().is_empty() => continue,
                Some(Ok(line)) => match serde_json::from_str(&line) {
                    Ok(msg) => return Some(msg),
                    Err(e) => { warn!(line = %line, "bad IPC message: {e}"); continue; }
                },
                Some(Err(e)) => { error!("IPC read error: {e}"); return None; }
                None => return None,
            }
        }
    }
}

pub struct PendingRpcs(HashMap<String, oneshot::Sender<Result<JsonValue, String>>>);

impl PendingRpcs {
    pub fn new() -> Self { Self(HashMap::new()) }

    pub fn register(&mut self, id: String) -> oneshot::Receiver<Result<JsonValue, String>> {
        let (tx, rx) = oneshot::channel();
        self.0.insert(id, tx);
        rx
    }

    pub fn resolve(&mut self, id: &str, result: Result<JsonValue, String>) {
        if let Some(tx) = self.0.remove(id) { let _ = tx.send(result); }
    }
}
