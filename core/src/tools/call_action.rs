use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg, check_permission};
use crate::governance::authority::has_permission;

pub struct CallActionTool;

#[async_trait]
impl Tool for CallActionTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "call_action".into(),
            description: concat!(
                "Execute a declared app action and return its raw result. Use list_actions to discover actions and input schemas. ",
                "Requires tool:call_action and effective app:<app>:invoke OR app:<app>:action:<action> permission. ",
                "Collection grants do not authorize this call; the target retains the caller's delegated permission ceiling. ",
                "Requires agent execution context. Not offered in the workflow palette or generic HTTP tool list; ",
                "workflow saves and generic HTTP execution reject this tool. Collection grants cannot enable it there."
            ).into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "Installed target app ID (argument name is app, not invoke_agent's app_id)" },
                    "action": { "type": "string", "description": "Action ID declared in the target app's manifest; discover with list_actions" },
                    "input": { "type": "object", "description": "Action input matching the action's inputSchema", "default": {} }
                },
                "required": ["app", "action"]
            }),
        }
    }

    fn requires_agent_context(&self) -> bool { true }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let app = str_arg(&ctx.args, "app")?;
        let action = str_arg(&ctx.args, "action")?;
        let input = ctx.args.get("input").cloned().unwrap_or(json!({}));

        // Same two grains as the HTTP RPC route. Checking only the fine key here
        // would deny a delegated agent whose principal holds the coarse one, so
        // the tool and the route would disagree about the same grant.
        if !has_permission(&ctx.permissions, &format!("app:{app}:invoke")) {
            check_permission(&ctx.permissions, &format!("app:{app}:action:{action}"))?;
        }

        let actions: Option<(JsonValue,)> = sqlx::query_as(
            "SELECT COALESCE(manifest->'actions', '[]'::jsonb) FROM rootcx_system.apps WHERE id = $1 AND status = 'installed'",
        )
        .bind(app)
        .fetch_optional(&ctx.pool)
        .await
        .map_err(|e| e.to_string())?;

        let (actions,) = actions.ok_or_else(|| format!("app '{app}' not found or not installed"))?;
        let declared = actions.as_array()
            .and_then(|arr| arr.iter().find(|a| a.get("id").and_then(|v| v.as_str()) == Some(action)))
            .ok_or_else(|| format!("action '{action}' not found in app '{app}'"))?;

        let caller = ctx.action_caller.as_ref().ok_or("action calling unavailable")?;
        let effective_uid = ctx.invoker_user_id.unwrap_or(ctx.user_id);
        let mut input = input;
        if let Some(agent_app) = ctx.core_bound_app_id.as_deref() {
            hand_over_files(&ctx.pool, declared, &mut input, agent_app, app, effective_uid).await?;
        }
        // Propagate the agent's pre-computed intersection so the target app's
        // RLS bounds the cross-app hop to grant(agent)∩perms(human) (Phase 6a).
        caller.call(app, action, input, effective_uid, &ctx.app_id, Some(ctx.permissions.clone())).await
    }
}

/// A top-level input declared `"format": "file"` may carry the file_id of a file
/// the person gave the agent. The target app receives its own copy, because an
/// app may only read its own storage. Other values pass through unchanged.
async fn hand_over_files(
    pool: &sqlx::PgPool, declared: &JsonValue, input: &mut JsonValue,
    agent_app: &str, target_app: &str, person: uuid::Uuid,
) -> Result<(), String> {
    use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};
    let Some(properties) = declared.pointer("/inputSchema/properties").and_then(JsonValue::as_object) else { return Ok(()) };
    for (key, schema) in properties {
        if schema.get("format").and_then(JsonValue::as_str) != Some("file") { continue; }
        let Some(id) = input.get(key).and_then(JsonValue::as_str).and_then(|v| v.parse::<uuid::Uuid>().ok()) else { continue };
        let given: Option<(i64,)> = sqlx::query_as(
            "SELECT size FROM rootcx_system.files WHERE id = $1 AND app_id = $2 AND uploaded_by = $3",
        ).bind(id).bind(agent_app).bind(person).fetch_optional(pool).await.map_err(|e| e.to_string())?;
        if given.is_none() { continue; }
        let file = PostgresBackend.get(pool, id, agent_app).await.map_err(|e| e.to_string())?;
        let copy = uuid::Uuid::new_v4();
        PostgresBackend.put(pool, copy, target_app, &file.name, &file.content_type, &file.content, Some(person))
            .await.map_err(|e| e.to_string())?;
        input[key] = json!(copy.to_string());
    }
    Ok(())
}
