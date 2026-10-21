use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext};
use crate::governance::authority::has_permission;

/// Same two grains as `call_action`, so metadata never advertises an action the caller cannot run.
pub(crate) fn invocable_actions(actions: &JsonValue, app: &str, permissions: &[String]) -> Vec<JsonValue> {
    let invoke = has_permission(permissions, &format!("app:{app}:invoke"));
    actions.as_array().into_iter().flatten().filter_map(|action| {
        let id = action.get("id")?.as_str()?;
        if !invoke && !has_permission(permissions, &format!("app:{app}:action:{id}")) {
            return None;
        }
        Some(json!({
            "id": id,
            "name": action.get("name"),
            "description": action.get("description"),
            "inputSchema": action.get("inputSchema"),
        }))
    }).collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocable_actions_follow_call_action_permission_grains() {
        let actions = json!([
            {"id": "create_item", "name": "Create", "description": "Creates.", "inputSchema": {"type": "object"}},
            {"id": "create", "name": "Short"},
            {"name": "missing id"},
            {"id": 7, "name": "non-string id"}
        ]);
        for (permissions, expected) in [
            (vec![], vec![]),
            (vec!["app:catalog:invoke"], vec!["create_item", "create"]),
            (vec!["app:catalog:action:create"], vec!["create"]),
            (vec!["app:catalog:action:create_item"], vec!["create_item"]),
            (vec!["app:other:invoke", "app:other:action:create_item"], vec![]),
            (vec!["app:catalog:items.read", "tool:call_action"], vec![]),
        ] {
            let permissions: Vec<String> = permissions.into_iter().map(String::from).collect();
            let ids: Vec<JsonValue> = invocable_actions(&actions, "catalog", &permissions)
                .into_iter().map(|action| action["id"].clone()).collect();
            assert_eq!(ids, expected, "{permissions:?}");
        }
        for malformed in [json!(null), json!({}), json!("create_item")] {
            assert!(
                invocable_actions(&malformed, "catalog", &["app:catalog:invoke".into()]).is_empty(),
                "{malformed}"
            );
        }
    }
}
