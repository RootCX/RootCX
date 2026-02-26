use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_shared_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg};

pub struct DescribeAppTool;

#[async_trait]
impl Tool for DescribeAppTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "describe_app".into(),
            description: "Get the full data contract (entities, fields, types, enums, references) for an app.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "The app ID to describe" }
                },
                "required": ["app"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let app = str_arg(&ctx.args, "app")?;
        let (name, dc): (String, JsonValue) = sqlx::query_as(
            "SELECT name, COALESCE(manifest->'dataContract', '[]'::jsonb) \
             FROM rootcx_system.apps WHERE id = $1",
        )
        .bind(app)
        .fetch_optional(&ctx.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("app '{app}' not found"))?;

        Ok(json!({ "app": app, "name": name, "dataContract": dc }))
    }
}
