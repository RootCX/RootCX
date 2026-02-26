use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_shared_types::ToolDescriptor;

use super::{Tool, ToolContext};

pub struct ListAppsTool;

#[async_trait]
impl Tool for ListAppsTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_apps".into(),
            description: "List all installed apps and their entity names.".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let rows: Vec<(String, String, JsonValue)> = sqlx::query_as(
            "SELECT id, name, COALESCE(manifest->'dataContract', '[]'::jsonb) \
             FROM rootcx_system.apps WHERE status = 'installed' ORDER BY name",
        )
        .fetch_all(&ctx.pool)
        .await
        .map_err(|e| e.to_string())?;

        Ok(rows.into_iter().map(|(id, name, dc)| {
            let entities: Vec<&str> = dc.as_array()
                .map(|arr| arr.iter().filter_map(|e| e.get("entityName").and_then(|v| v.as_str())).collect())
                .unwrap_or_default();
            json!({ "id": id, "name": name, "entities": entities })
        }).collect())
    }
}
