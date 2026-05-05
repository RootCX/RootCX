use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext};

pub struct ListActionsTool;

#[async_trait]
impl Tool for ListActionsTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_actions".into(),
            description: "List actions exposed by installed apps. Actions are callable via call_action. Returns app ID, action definitions (id, name, description, inputSchema), and usage instructions.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "Filter by app ID (optional — omit to list all)" }
                }
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let filter_app = ctx.args.get("app").and_then(|v| v.as_str());

        let rows: Vec<(String, String, JsonValue, Option<String>)> = sqlx::query_as(
            "SELECT id, name, COALESCE(manifest->'actions', '[]'::jsonb), manifest->>'instructions' \
             FROM rootcx_system.apps \
             WHERE status = 'installed' AND ($1::text IS NULL OR id = $1) \
               AND jsonb_array_length(COALESCE(manifest->'actions', '[]'::jsonb)) > 0 \
             ORDER BY name",
        )
        .bind(filter_app)
        .fetch_all(&ctx.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(rows.into_iter().map(|(id, name, actions, instructions)| {
            let mut entry = json!({ "appId": id, "name": name, "actions": actions });
            if let Some(inst) = instructions {
                entry["instructions"] = json!(inst);
            }
            entry
        }).collect())
    }
}
