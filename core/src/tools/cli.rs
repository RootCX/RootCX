use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use tokio::process::Command;
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg};

const CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub struct CliTool {
    command: String,
    args_prefix: Vec<String>,
    descriptor: ToolDescriptor,
}

impl CliTool {
    pub fn new(name: String, command: String, args_prefix: Vec<String>) -> Self {
        let descriptor = ToolDescriptor {
            name: name.clone(),
            description: format!("Execute {command} CLI commands"),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "CLI arguments to pass" }
                },
                "required": ["command"]
            }),
        };
        Self { command, args_prefix, descriptor }
    }
}

#[async_trait]
impl Tool for CliTool {
    fn descriptor(&self) -> ToolDescriptor { self.descriptor.clone() }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let raw = str_arg(&ctx.args, "command")?;
        let parts: Vec<&str> = raw.split_whitespace().collect();

        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args_prefix);
        cmd.args(&parts);

        let output = tokio::time::timeout(CMD_TIMEOUT, cmd.output())
            .await
            .map_err(|_| format!("timed out after {}s", CMD_TIMEOUT.as_secs()))?
            .map_err(|e| format!("spawn failed: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output.status.code().unwrap_or(-1);

        if code != 0 && stdout.is_empty() {
            return Err(stderr.trim().to_string());
        }

        Ok(json!({
            "stdout": stdout.trim_end(),
            "stderr": stderr.trim_end(),
            "exitCode": code
        }))
    }
}
