use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use tracing::info;

use crate::RuntimeError;
use rootcx_types::ToolDescriptor;

use super::transport::StdioTransport;

const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Deserialize)]
struct ToolsListResult {
    tools: Vec<McpToolDef>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct McpToolDef {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    input_schema: JsonValue,
}

#[derive(Deserialize)]
struct ToolCallResult {
    content: Vec<ToolCallContent>,
    #[serde(default)]
    is_error: bool,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ToolCallContent {
    Text { text: String },
    #[serde(other)]
    Other,
}

pub struct McpClient {
    transport: Arc<StdioTransport>,
    pub server_name: String,
}

impl McpClient {
    pub async fn connect_stdio(
        server_name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        working_dir: Option<&Path>,
    ) -> Result<Self, RuntimeError> {
        let transport = Arc::new(StdioTransport::spawn(command, args, env, working_dir).await?);
        let client = Self { transport, server_name: server_name.into() };
        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<(), RuntimeError> {
        let result = self.transport.request("initialize", Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "rootcx-core", "version": env!("CARGO_PKG_VERSION") }
        }))).await?;

        let version = result.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("unknown");
        info!(server = %self.server_name, protocol = %version, "MCP initialized");
        self.transport.notify("notifications/initialized").await
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolDescriptor>, RuntimeError> {
        let result = self.transport.request("tools/list", Some(json!({}))).await?;
        let list: ToolsListResult = serde_json::from_value(result)
            .map_err(|e| RuntimeError::Mcp(format!("tools/list parse: {e}")))?;

        Ok(list.tools.into_iter().map(|t| ToolDescriptor {
            name: t.name,
            description: t.description.unwrap_or_default(),
            input_schema: t.input_schema,
        }).collect())
    }

    pub async fn call_tool(&self, name: &str, args: &JsonValue) -> Result<JsonValue, String> {
        let result = self.transport.request("tools/call", Some(json!({ "name": name, "arguments": args })))
            .await.map_err(|e| e.to_string())?;
        let call: ToolCallResult = serde_json::from_value(result).map_err(|e| format!("tools/call parse: {e}"))?;

        let text: String = call.content.iter().filter_map(|c| match c {
            ToolCallContent::Text { text } => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("\n");

        if call.is_error { Err(text) } else { Ok(json!(text)) }
    }

    pub async fn shutdown(&self) {
        self.transport.kill().await;
    }
}
