use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext};
use crate::governance::action_invocation::{Declaration, authorize};

/// The declared actions this caller may run, decided by `action_invocation`, so
/// a listing never offers what a call would refuse.
pub(crate) fn invocable<'a>(
    actions: &'a JsonValue, app: &'a str, permissions: &'a [String], assistant: bool,
) -> impl Iterator<Item = (&'a str, Declaration)> + 'a {
    actions.as_array().into_iter().flatten().filter_map(move |action| {
        let id = action.get("id")?.as_str()?;
        let declaration = Declaration::from_definition(action.clone());
        authorize(app, id, Some(&declaration), permissions, assistant).ok()?;
        Some((id, declaration))
    })
}

/// [`invocable`] as listings show it: each action with its declared hints.
pub(crate) fn invocable_actions(actions: &JsonValue, app: &str, permissions: &[String], assistant: bool) -> Vec<JsonValue> {
    invocable(actions, app, permissions, assistant).map(|(id, declaration)| {
        let action = &declaration.definition;
        let mut entry = json!({
            "id": id,
            "name": action.get("name"),
            "description": action.get("description"),
            "inputSchema": action.get("inputSchema"),
        });
        if let Some(annotations) = action.get("annotations") {
            entry["annotations"] = annotations.clone();
        }
        entry
    }).collect()
}

pub struct ListActionsTool;

#[async_trait]
impl Tool for ListActionsTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_actions".into(),
            description: "List actions exposed by installed apps. Agents run them with call_action; over MCP each is its own tool, <app id>__<action id>. Returns app ID, action definitions (id, name, description, inputSchema, optional annotations), and usage instructions.".into(),
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
            let actions = invocable_actions(&actions, &id, &ctx.permissions, ctx.is_assistant());
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
            let ids: Vec<JsonValue> = invocable_actions(&actions, "catalog", &permissions, false)
                .into_iter().map(|action| action["id"].clone()).collect();
            assert_eq!(ids, expected, "{permissions:?}");
        }
        let annotated = json!([
            {"id": "delete_item", "annotations": {"destructiveHint": true}},
            {"id": "list_items"}
        ]);
        let listed = invocable_actions(&annotated, "catalog", &["app:catalog:invoke".into()], false);
        assert_eq!(listed[0]["annotations"], json!({"destructiveHint": true}), "declared hints reach callers");
        assert!(listed[1].get("annotations").is_none(), "no hint is invented for an undeclared action");
        let governed = json!([
            {"id": "pay", "authority": {"data": {"invoices": ["update"]}}},
            {"id": "send", "humanOnly": true},
            {"id": "read"}
        ]);
        let invoke = ["app:catalog:invoke".to_string()];
        let ids = |assistant| invocable_actions(&governed, "catalog", &invoke, assistant)
            .into_iter().map(|action| action["id"].clone()).collect::<Vec<_>>();
        assert_eq!(ids(false), vec![json!("send"), json!("read")], "an approved action needs its own grain");
        assert_eq!(ids(true), vec![json!("read")], "assistants never see humanOnly actions");
        for malformed in [json!(null), json!({}), json!("create_item")] {
            assert!(
                invocable_actions(&malformed, "catalog", &["app:catalog:invoke".into()], false).is_empty(),
                "{malformed}"
            );
        }
    }
}
