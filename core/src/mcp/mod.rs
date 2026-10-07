mod client;
mod transport;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::RuntimeError;
use crate::tools::{Tool, ToolContext, ToolRegistry};
use crate::tools::cli::CliTool;
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
    cli_names: RwLock<Vec<String>>,
    tool_registry: Arc<ToolRegistry>,
    bun_bin: PathBuf,
}

impl McpManager {
    pub fn new(tool_registry: Arc<ToolRegistry>, bun_bin: PathBuf) -> Self {
        Self {
            clients: RwLock::new(HashMap::new()),
            cli_names: RwLock::new(Vec::new()),
            tool_registry,
            bun_bin,
        }
    }

    pub async fn start_server(&self, config: &McpServerConfig, env: &HashMap<String, String>) -> Result<Vec<String>, RuntimeError> {
        if crate::worker_sandbox::enabled()? {
            return Err(RuntimeError::Mcp("custom MCP connections are unavailable in isolated deployments until their process transports are qualified".into()));
        }
        let name = &config.name;

        if let McpTransport::Cli { install } = &config.transport {
            // Run install command once
            let parts: Vec<&str> = install.split_whitespace().collect();
            if parts.is_empty() {
                return Err(RuntimeError::Mcp("empty install command".into()));
            }
            let output = tokio::process::Command::new(parts[0])
                .args(&parts[1..])
                .envs(env)
                .output()
                .await
                .map_err(|e| RuntimeError::Mcp(format!("install failed: {e}")))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(RuntimeError::Mcp(format!("install failed: {}", stderr.trim())));
            }
            info!(server = %name, install = %install, "CLI install complete");

            let tool = CliTool::new(name.clone(), name.clone(), vec![]);
            self.tool_registry.register(tool);
            self.cli_names.write().await.push(name.clone());
            info!(server = %name, "CLI tool registered");
            return Ok(vec![name.clone()]);
        }

        let client = match &config.transport {
            McpTransport::Stdio { command, args } => {
                Arc::new(McpClient::connect_stdio(name, command, args, env, None).await?)
            }
            #[allow(deprecated)]
            McpTransport::Http { url, headers } | McpTransport::Sse { url, headers } => {
                let mut args = vec!["@rootcx/mcp-bridge".into(), url.clone()];
                for (k, v) in headers {
                    args.push("--header".into());
                    args.push(format!("{k}:{v}"));
                }
                let runner = self.bun_bin.to_string_lossy().into_owned();
                Arc::new(McpClient::connect_stdio(name, &runner, &[&["x".into()], args.as_slice()].concat(), env, None).await?)
            }
            McpTransport::Cli { .. } => unreachable!(),
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
        }
        self.cli_names.write().await.retain(|n| n != name);
        self.tool_registry.unregister_prefix(&format!("{name}_"));
        // CLI tools are registered directly by name (no prefix)
        self.tool_registry.unregister(name);
        info!(server = %name, "server stopped");
        Ok(())
    }

    pub async fn stop_all(&self) {
        let mcp_names: Vec<String> = self.clients.read().await.keys().cloned().collect();
        let cli_names: Vec<String> = self.cli_names.read().await.clone();
        for name in mcp_names.iter().chain(cli_names.iter()) {
            if let Err(e) = self.stop_server(name).await {
                error!(server = %name, "stop error: {e}");
            }
        }
    }

    pub fn tool_registry(&self) -> &Arc<ToolRegistry> { &self.tool_registry }

    pub async fn is_running(&self, name: &str) -> bool {
        self.clients.read().await.contains_key(name)
            || self.cli_names.read().await.iter().any(|n| n == name)
    }
}


#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    #[ignore = "requires ROOTCX_WORKER_SANDBOX=srt on Linux"]
    async fn isolated_core_rejects_all_mcp_process_transports_before_execution() {
        assert!(crate::worker_sandbox::enabled().unwrap());
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("escaped");
        let fake_bun = directory.path().join("bun");
        std::fs::write(&fake_bun, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        std::fs::set_permissions(&fake_bun, std::fs::Permissions::from_mode(0o755)).unwrap();
        let manager = McpManager::new(Arc::new(ToolRegistry::default()), fake_bun);
        for transport in [
            McpTransport::Cli { install: format!("touch {}", marker.display()) },
            McpTransport::Stdio { command: "touch".into(), args: vec![marker.to_string_lossy().into_owned()] },
            McpTransport::Http { url: "https://example.invalid/mcp".into(), headers: HashMap::new() },
        ] {
            let error = manager.start_server(&McpServerConfig { name: "escape".into(), transport }, &HashMap::new()).await.unwrap_err();
            assert!(error.to_string().contains("unavailable in isolated deployments"));
            assert!(!marker.exists(), "MCP must refuse before an install, shell or bunx process runs");
            assert!(!manager.is_running("escape").await);
        }
    }
}
