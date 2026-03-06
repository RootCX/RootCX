mod client;
mod transport;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::RuntimeError;
use crate::tools::{Tool, ToolContext, ToolRegistry};
use client::McpClient;
use rootcx_types::{McpServerConfig, McpTransport, ToolDescriptor};

struct McpTool {
    remote_name: String,
    descriptor: ToolDescriptor,
    client: Arc<McpClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn descriptor(&self) -> ToolDescriptor { self.descriptor.clone() }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        self.client.call_tool(&self.remote_name, &ctx.args).await
    }
}

pub struct McpManager {
    clients: RwLock<HashMap<String, Arc<McpClient>>>,
    tool_registry: Arc<ToolRegistry>,
}

impl McpManager {
    pub fn new(tool_registry: Arc<ToolRegistry>) -> Self {
        Self { clients: RwLock::new(HashMap::new()), tool_registry }
    }

    pub async fn start_server(&self, config: &McpServerConfig, env_overrides: &HashMap<String, String>) -> Result<Vec<String>, RuntimeError> {
        let name = &config.name;
        let mut env = config.env.clone();
        env.extend(env_overrides.clone());

        let client = match &config.transport {
            McpTransport::Stdio { command, args } => {
                Arc::new(McpClient::connect_stdio(name, command, args, &env, None).await?)
            }
            McpTransport::Sse { .. } => {
                return Err(RuntimeError::Mcp("SSE transport not yet implemented".into()));
            }
        };

        let tools = client.list_tools().await?;
        let registered: Vec<String> = tools.into_iter().map(|tool| {
            let remote_name = tool.name.clone();
            let namespaced = format!("{name}_{remote_name}");
            self.tool_registry.register(McpTool {
                remote_name,
                descriptor: ToolDescriptor {
                    name: namespaced.clone(),
                    description: tool.description,
                    input_schema: tool.input_schema,
                },
                client: client.clone(),
            });
            namespaced
        }).collect();

        info!(server = %name, tools = registered.len(), "MCP server started");
        self.clients.write().await.insert(name.clone(), client);
        Ok(registered)
    }

    pub async fn stop_server(&self, name: &str) -> Result<(), RuntimeError> {
        if let Some(c) = self.clients.write().await.remove(name) {
            c.shutdown().await;
            self.tool_registry.unregister_prefix(&format!("{name}_"));
            info!(server = %name, "MCP server stopped");
        }
        Ok(())
    }

    pub async fn stop_all(&self) {
        let names: Vec<String> = self.clients.read().await.keys().cloned().collect();
        for name in names {
            if let Err(e) = self.stop_server(&name).await {
                error!(server = %name, "stop error: {e}");
            }
        }
    }

    pub async fn is_running(&self, name: &str) -> bool {
        self.clients.read().await.contains_key(name)
    }
}
