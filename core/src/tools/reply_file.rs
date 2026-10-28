use super::{Tool, ToolContext};
use async_trait::async_trait;
use rootcx_types::ToolDescriptor;
use serde_json::{json, Value};

pub struct ReplyFileTool;
#[async_trait]
impl Tool for ReplyFileTool {
    fn requires_agent_context(&self) -> bool { true }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "reply_file".into(),
            description: "Attach a stored PDF to the current channel conversation, only when the person asks to receive it here. Uses app_id and file_id returned by an app action. No recipient argument, public link, customer delivery or business status change. Fails outside a supported channel; only claim delivery after success.".into(),
            input_schema: json!({"type":"object","properties":{"app_id":{"type":"string"},"file_id":{"type":"string","format":"uuid"}},"required":["app_id","file_id"],"additionalProperties":false}),
        }
    }
    async fn execute(&self, ctx: &ToolContext) -> Result<Value, String> {
        crate::extensions::channels::reply_file::reply(ctx).await
    }
}
