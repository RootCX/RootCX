use std::collections::HashMap;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::io::{AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::oneshot;
use tokio_util::codec::{FramedRead, LinesCodec};
use tracing::error;

use crate::RuntimeError;

const MAX_LINE_LENGTH: usize = 1_048_576;

fn ipc_err(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Ipc(e.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcCaller {
    pub user_id: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentBootConfig {
    pub tool_descriptors: Vec<rootcx_types::ToolDescriptor>,
    pub max_turns: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LlmModelRef {
    pub provider: String,
    pub model: String,
}

/// A file attachment forwarded to the agent worker for multimodal LLM input.
/// `url` is a one-time nonce download URL — worker fetches bytes via HTTP.
#[derive(Debug, Clone, Serialize)]
pub struct FileAttachment {
    pub name: String,
    pub content_type: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentInvokePayload {
    pub invoke_id: String,
    pub session_id: String,
    pub message: String,
    pub history: Vec<JsonValue>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_sub_invoke: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmModelRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invoker_user_id: Option<uuid::Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<FileAttachment>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutboundMessage {
    Discover {
        app_id: String,
        runtime_url: String,
        database_url: String,
        #[serde(skip_serializing_if = "HashMap::is_empty")]
        credentials: HashMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        agent_config: Option<AgentBootConfig>,
    },
    Rpc { id: String, method: String, params: JsonValue, caller: Option<RpcCaller> },
    Job { id: String, payload: JsonValue, #[serde(skip_serializing_if = "Option::is_none")] caller: Option<RpcCaller> },
    AgentInvoke(AgentInvokePayload),
    AgentToolResult {
        invoke_id: String,
        call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<JsonValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    CollectionOpResult {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<JsonValue>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    StorageUploadUrl {
        id: String,
        url: String,
    },
    Shutdown,
}

/// Worker IPC protocol version.
///
/// Versioning rules (see also `backend_prelude.js`):
///
/// * **v1 (legacy)** — worker manages its own stdin listener, writes the
///   IPC protocol by hand, does not call `serve()`. Any worker that omits
///   the `protocol` field in its `discover` response is implicitly v1.
///   Supported indefinitely: existing client apps ship on v1.
///
/// * **v2** — worker calls `globalThis.serve({ rpc, onStart, onJob,
///   onShutdown })` from the injected prelude. The prelude owns the single
///   stdin dispatcher and builds a `ctx` (`collection`, `uploadFile`, …).
///   The worker MUST announce `protocol: 2` in its `discover` response.
///
/// Adding a new version = add a const, bump the scaffold template, teach
/// the supervisor which messages are safe to send based on the negotiated
/// version. Never remove a version without an explicit migration plan.
pub const LATEST_PROTOCOL_VERSION: u32 = 2;

fn default_protocol_version() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboundMessage {
    /// Worker handshake. `protocol` advertises the IPC version the worker
    /// speaks. Absent = v1 (legacy, pre-`serve()` apps). Extra fields
    /// (`methods`, …) are accepted and currently ignored by the Core but
    /// may be used by future versions — see `LATEST_PROTOCOL_VERSION`.
    Discover {
        #[serde(default = "default_protocol_version")]
        protocol: u32,
    },
    RpcResponse {
        id: String,
        #[serde(default)]
        result: Option<JsonValue>,
        #[serde(default)]
        error: Option<String>,
    },
    JobResult {
        id: String,
        #[serde(default)]
        error: Option<String>,
    },
    Log {
        #[serde(default = "default_level")]
        level: String,
        message: String,
    },
    AgentChunk {
        invoke_id: String,
        delta: String,
    },
    AgentDone {
        invoke_id: String,
        response: String,
        #[serde(default)]
        tokens: Option<u64>,
    },
    AgentError {
        invoke_id: String,
        error: String,
    },
    AgentToolCall {
        invoke_id: String,
        call_id: String,
        tool_name: String,
        #[serde(default)]
        args: JsonValue,
    },
    AgentSessionCompacted {
        invoke_id: String,
        summary: String,
    },
    Event {
        name: String,
        #[serde(default)]
        data: JsonValue,
    },
    CollectionOp {
        id: String,
        op: String,
        entity: String,
        #[serde(default)]
        data: JsonValue,
    },
    StorageUpload {
        id: String,
        name: String,
        #[serde(default = "default_content_type")]
        content_type: String,
        #[serde(default)]
        size: usize,
    },
}

fn default_content_type() -> String {
    "application/octet-stream".into()
}

fn default_level() -> String {
    "info".into()
}

#[derive(Debug)]
pub enum IpcEvent {
    Message(InboundMessage),
    Output(String),
}

pub struct IpcWriter {
    stdin: ChildStdin,
}

impl IpcWriter {
    pub fn new(stdin: ChildStdin) -> Self {
        Self { stdin }
    }

    pub async fn send(&mut self, msg: &OutboundMessage) -> Result<(), RuntimeError> {
        let mut line = serde_json::to_string(msg).map_err(ipc_err)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.map_err(ipc_err)?;
        self.stdin.flush().await.map_err(ipc_err)
    }
}

pub struct IpcReader {
    lines: FramedRead<BufReader<Box<dyn AsyncRead + Unpin + Send>>, LinesCodec>,
}

impl IpcReader {
    pub fn new(reader: impl AsyncRead + Unpin + Send + 'static) -> Self {
        Self {
            lines: FramedRead::new(
                BufReader::new(Box::new(reader) as Box<dyn AsyncRead + Unpin + Send>),
                LinesCodec::new_with_max_length(MAX_LINE_LENGTH),
            ),
        }
    }

    pub async fn recv(&mut self) -> Option<IpcEvent> {
        loop {
            match self.lines.next().await {
                Some(Ok(line)) if line.trim().is_empty() => continue,
                Some(Ok(line)) => match serde_json::from_str(&line) {
                    Ok(msg) => return Some(IpcEvent::Message(msg)),
                    Err(_) => return Some(IpcEvent::Output(line)),
                },
                Some(Err(e)) => {
                    error!("IPC read error: {e}");
                    return None;
                }
                None => return None,
            }
        }
    }
}

#[derive(Default)]
pub struct PendingRpcs(HashMap<String, oneshot::Sender<Result<JsonValue, String>>>);

impl PendingRpcs {
    pub fn register(&mut self, id: String) -> oneshot::Receiver<Result<JsonValue, String>> {
        let (tx, rx) = oneshot::channel();
        self.0.insert(id, tx);
        rx
    }

    pub fn resolve(&mut self, id: &str, result: Result<JsonValue, String>) {
        if let Some(tx) = self.0.remove(id) {
            let _ = tx.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn outbound_messages_carry_type_tag() {
        let cases: Vec<(OutboundMessage, &str)> = vec![
            (OutboundMessage::Discover { app_id: "a".into(), runtime_url: "r".into(), database_url: "postgres://localhost:5480/postgres".into(), credentials: HashMap::new(), agent_config: None }, "discover"),
            (OutboundMessage::Rpc { id: "r1".into(), method: "echo".into(), params: json!({}), caller: None }, "rpc"),
            (OutboundMessage::Job { id: "j1".into(), payload: json!({}), caller: None }, "job"),
            (OutboundMessage::CollectionOpResult { id: "c1".into(), result: Some(json!({})), error: None }, "collection_op_result"),
            (OutboundMessage::Shutdown, "shutdown"),
            (OutboundMessage::AgentToolResult {
                invoke_id: "i1".into(), call_id: "c1".into(),
                result: Some(json!({"data": []})), error: None,
            }, "agent_tool_result"),
        ];
        for (msg, expected_type) in cases {
            let v: JsonValue = serde_json::to_value(&msg).unwrap();
            assert_eq!(v["type"], expected_type, "wrong type tag for {expected_type}");
        }
    }

    #[test]
    fn inbound_discover_without_protocol_defaults_to_v1() {
        // Legacy apps (pre-`serve()`) omit `protocol` entirely. They must
        // still parse cleanly and fall through to protocol v1.
        let msg: InboundMessage = serde_json::from_str(r#"{"type":"discover"}"#).unwrap();
        let InboundMessage::Discover { protocol } = msg else { panic!("expected Discover") };
        assert_eq!(protocol, 1);
    }

    #[test]
    fn inbound_discover_ignores_extra_fields_but_reads_protocol() {
        // Workers MAY send `methods` (ignored today) alongside `protocol`.
        let raw = r#"{"type":"discover","protocol":2,"methods":["ping"]}"#;
        let msg: InboundMessage = serde_json::from_str(raw).unwrap();
        let InboundMessage::Discover { protocol } = msg else { panic!("expected Discover") };
        assert_eq!(protocol, 2);
    }

    #[test]
    fn inbound_discover_accepts_future_protocol_versions() {
        // Unknown (higher) protocol versions deserialize successfully; it
        // is the supervisor's job — not the parser's — to decide whether
        // it can speak to that worker. This keeps forward-compat cheap.
        let raw = r#"{"type":"discover","protocol":99}"#;
        let msg: InboundMessage = serde_json::from_str(raw).unwrap();
        let InboundMessage::Discover { protocol } = msg else { panic!("expected Discover") };
        assert_eq!(protocol, 99);
    }

    #[test]
    fn inbound_rpc_response_success() {
        let msg: InboundMessage = serde_json::from_str(r#"{"type":"rpc_response","id":"1","result":42}"#).unwrap();
        let InboundMessage::RpcResponse { id, result, error } = msg else { panic!("expected RpcResponse") };
        assert_eq!(id, "1");
        assert_eq!(result, Some(json!(42)));
        assert_eq!(error, None);
    }

    #[test]
    fn inbound_rpc_response_error() {
        let msg: InboundMessage =
            serde_json::from_str(r#"{"type":"rpc_response","id":"1","error":"not found"}"#).unwrap();
        let InboundMessage::RpcResponse { error, .. } = msg else { panic!("expected RpcResponse") };
        assert_eq!(error, Some("not found".into()));
    }

    #[test]
    fn inbound_job_result_success() {
        let msg: InboundMessage = serde_json::from_str(r#"{"type":"job_result","id":"j1"}"#).unwrap();
        let InboundMessage::JobResult { id, error } = msg else { panic!("expected JobResult") };
        assert_eq!(id, "j1");
        assert_eq!(error, None);
    }

    #[test]
    fn inbound_job_result_error() {
        let msg: InboundMessage = serde_json::from_str(r#"{"type":"job_result","id":"j1","error":"timeout"}"#).unwrap();
        let InboundMessage::JobResult { error, .. } = msg else { panic!("expected JobResult") };
        assert_eq!(error, Some("timeout".into()));
    }

    #[test]
    fn inbound_log_default_level() {
        let msg: InboundMessage = serde_json::from_str(r#"{"type":"log","message":"hi"}"#).unwrap();
        let InboundMessage::Log { level, message } = msg else { panic!("expected Log") };
        assert_eq!(level, "info");
        assert_eq!(message, "hi");
    }

    #[test]
    fn inbound_log_explicit_level() {
        let msg: InboundMessage = serde_json::from_str(r#"{"type":"log","level":"error","message":"hi"}"#).unwrap();
        let InboundMessage::Log { level, message } = msg else { panic!("expected Log") };
        assert_eq!(level, "error");
        assert_eq!(message, "hi");
    }

    #[test]
    fn inbound_invalid_type_fails() {
        let result = serde_json::from_str::<InboundMessage>(r#"{"type":"unknown"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn inbound_agent_tool_call() {
        let msg: InboundMessage = serde_json::from_str(
            r#"{"type":"agent_tool_call","invoke_id":"i1","call_id":"c1","tool_name":"query_data","args":{"entity":"Users"}}"#
        ).unwrap();
        let InboundMessage::AgentToolCall { invoke_id, call_id, tool_name, args } = msg else { panic!("expected AgentToolCall") };
        assert_eq!(invoke_id, "i1");
        assert_eq!(call_id, "c1");
        assert_eq!(tool_name, "query_data");
        assert_eq!(args, json!({"entity": "Users"}));
    }

    #[test]
    fn inbound_agent_session_compacted() {
        let msg: InboundMessage = serde_json::from_str(
            r#"{"type":"agent_session_compacted","invoke_id":"i1","summary":"conversation about users"}"#
        ).unwrap();
        let InboundMessage::AgentSessionCompacted { invoke_id, summary } = msg else { panic!("expected AgentSessionCompacted") };
        assert_eq!(invoke_id, "i1");
        assert_eq!(summary, "conversation about users");
    }

    #[test]
    fn inbound_collection_op() {
        let msg: InboundMessage = serde_json::from_str(
            r#"{"type":"collection_op","id":"c1","op":"insert","entity":"docs","data":{"title":"test"}}"#
        ).unwrap();
        let InboundMessage::CollectionOp { id, op, entity, data } = msg else { panic!("expected CollectionOp") };
        assert_eq!(id, "c1");
        assert_eq!(op, "insert");
        assert_eq!(entity, "docs");
        assert_eq!(data, json!({"title": "test"}));
    }

    #[test]
    fn pending_rpcs_register_resolve_ok() {
        let mut rpcs = PendingRpcs::default();
        let mut rx = rpcs.register("r1".into());
        rpcs.resolve("r1", Ok(json!(42)));
        assert_eq!(rx.try_recv().unwrap(), Ok(json!(42)));
    }

    #[test]
    fn pending_rpcs_register_resolve_err() {
        let mut rpcs = PendingRpcs::default();
        let mut rx = rpcs.register("r1".into());
        rpcs.resolve("r1", Err("fail".into()));
        assert_eq!(rx.try_recv().unwrap(), Err("fail".to_string()));
    }

    #[test]
    fn pending_rpcs_resolve_unknown_noop() {
        let mut rpcs = PendingRpcs::default();
        rpcs.resolve("nonexistent", Ok(json!(null)));
        // no panic — passes if we reach here
    }

    #[test]
    fn pending_rpcs_register_replaces() {
        let mut rpcs = PendingRpcs::default();
        let mut rx1 = rpcs.register("r1".into());
        let _rx2 = rpcs.register("r1".into());
        // First sender was dropped when the key was replaced
        assert!(rx1.try_recv().is_err());
    }

    #[test]
    fn pending_rpcs_multiple_independent() {
        let mut rpcs = PendingRpcs::default();
        let mut rx_a = rpcs.register("a".into());
        let mut rx_b = rpcs.register("b".into());

        // resolve "b" first, then "a"
        rpcs.resolve("b", Ok(json!("beta")));
        rpcs.resolve("a", Ok(json!("alpha")));

        assert_eq!(rx_a.try_recv().unwrap(), Ok(json!("alpha")));
        assert_eq!(rx_b.try_recv().unwrap(), Ok(json!("beta")));
    }
}
