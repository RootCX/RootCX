use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext};
use crate::extensions::integrations::routes::query_installed_integrations;

pub struct ListIntegrationsTool;

#[async_trait]
impl Tool for ListIntegrationsTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_integrations".into(),
            description: "List available integrations with their actions and schemas. Use this to discover what external services (email, CRM, messaging, etc.) can be used in apps via the SDK's useIntegration hook.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        query_installed_integrations(&ctx.pool).await
            .map(JsonValue::Array)
            .map_err(|e: sqlx::Error| e.to_string())
    }
}
