use super::{Tool, ToolContext};
use async_trait::async_trait;
use rootcx_types::ToolDescriptor;
use serde_json::{json, Value};

pub struct ReplyNowTool;
#[async_trait]
impl Tool for ReplyNowTool {
    fn requires_agent_context(&self) -> bool { true }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "reply_now".into(),
            description: "Send one short message to the person in the current channel conversation right away, without ending your work. Use it only to confirm, in one sentence, a message they sent while you were working. Not for results, questions or customers; your final answer still follows. Fails outside a channel conversation.".into(),
            input_schema: json!({"type":"object","properties":{"text":{"type":"string","minLength":1,"maxLength":500}},"required":["text"],"additionalProperties":false}),
        }
    }
    async fn execute(&self, ctx: &ToolContext) -> Result<Value, String> {
        crate::extensions::channels::reply::reply_now(ctx).await
    }
}
