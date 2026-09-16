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
            description: concat!(
                "Delegate a task to another agent and return {agent, response}. ",
                "Requires tool:invoke_agent and effective app:<app_id>:invoke permission. ",
                "The child is bounded by the parent's effective permissions and task scope; collection grants do not authorize invocation. ",
                "Requires agent execution context. Not offered in the workflow palette or generic HTTP tool list; ",
                "workflow saves and generic HTTP execution reject this tool. Collection grants cannot enable it there."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app_id": { "type": "string", "description": "Target agent's app ID, different from the calling app (argument name is app_id, not call_action's app)" },
                    "message": { "type": "string", "description": "The message/task to send to the agent" }
                },
                "required": ["app_id", "message"]
            }),
        }
    }

    fn requires_agent_context(&self) -> bool { true }

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
