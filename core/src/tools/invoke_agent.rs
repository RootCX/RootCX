use std::sync::LazyLock;

use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg};

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(||
    reqwest::Client::builder().no_proxy().build().expect("reqwest client")
);

pub struct InvokeAgentTool;

#[async_trait]
impl Tool for InvokeAgentTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "invoke_agent".into(),
            description: "Invoke another agent and return its response. Use this to delegate tasks to specialized agents.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app_id": { "type": "string", "description": "The app ID of the agent to invoke" },
                    "message": { "type": "string", "description": "The message/task to send to the agent" }
                },
                "required": ["app_id", "message"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let target = str_arg(&ctx.args, "app_id")?;
        let message = str_arg(&ctx.args, "message")?;

        if target == ctx.app_id {
            return Err("cannot invoke self".into());
        }

        let res = HTTP
            .post(format!("{}/api/v1/apps/{}/agent/invoke", ctx.runtime_url, target))
            .bearer_auth(&ctx.auth_token)
            .json(&json!({ "message": message }))
            .send()
            .await
            .map_err(|e| format!("invoke_agent: {e}"))?;

        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(format!("invoke_agent ({status}): {body}"));
        }

        let text = res.text().await.map_err(|e| e.to_string())?;
        let mut response = String::new();

        for line in text.lines() {
            if !line.starts_with("data: ") { continue; }
            if let Ok(data) = serde_json::from_str::<JsonValue>(&line[6..]) {
                if let Some(delta) = data["delta"].as_str() { response.push_str(delta); }
                if let Some(full) = data["response"].as_str() { response = full.to_string(); }
                if let Some(err) = data["error"].as_str() { return Err(format!("agent error: {err}")); }
            }
        }

        Ok(json!({ "agent": target, "response": response }))
    }
}
