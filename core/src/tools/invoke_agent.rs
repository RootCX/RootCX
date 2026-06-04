use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, check_permission, str_arg};

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
        // PEP-hop: the effective authority (grant(agent) ∩ perms(human)) must
        // include invoke rights on the target, else the human could trigger an
        // agent they cannot invoke. ctx.permissions is that intersection.
        check_permission(&ctx.permissions, &format!("app:{target}:invoke"))?;
        let dispatch = ctx.agent_dispatch.as_ref().ok_or("sub-agent dispatch unavailable")?;
        // ctx.permissions is THIS (parent) agent's frozen authority; pass it so
        // the child narrows against the parent, not the human (the chain stays
        // monotone non-increasing).
        let response = dispatch.dispatch(&ctx.pool, &ctx.app_id, target, message, ctx.stream_tx.clone(), ctx.invoker_user_id, ctx.permissions.clone(), ctx.task_scope.clone()).await?;
        Ok(json!({ "agent": target, "response": response }))
    }
}
