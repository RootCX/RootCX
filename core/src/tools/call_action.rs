use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg};
use crate::governance::action_invocation::{Declaration, authorize, dispatch};
use crate::ipc::RpcCaller;

pub struct CallActionTool;

#[async_trait]
impl Tool for CallActionTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "call_action".into(),
            description: concat!(
                "Execute a declared app action and return its raw result. Use list_actions to discover actions and input schemas. ",
                "Requires tool:call_action and effective app:<app>:invoke OR app:<app>:action:<action> permission; ",
                "an action with approved authority requires app:<app>:action:<action>. ",
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
        let mut input = ctx.args.get("input").cloned().unwrap_or(json!({}));
        let dispatcher = ctx.action_caller.as_ref().ok_or("action calling unavailable")?;
        let declaration = Declaration::load(&ctx.pool, app, action).await.map_err(|e| e.to_string())?;
        // Authorized before any file is copied into the target app.
        authorize(app, action, declaration.as_ref(), &ctx.permissions, true).map_err(|e| e.to_string())?;
        let effective_uid = ctx.invoker_user_id.unwrap_or(ctx.user_id);
        if let (Some(agent_app), Some(declared)) = (ctx.core_bound_app_id.as_deref(), declaration.as_ref()) {
            hand_over_files(&ctx.pool, &declared.definition, &mut input, agent_app, app, effective_uid).await?;
        }
        // The agent's pre-computed intersection rides along so the target app's
        // RLS bounds the cross-app hop to grant(agent)∩perms(human).
        let caller = RpcCaller {
            user_id: effective_uid.to_string(),
            email: String::new(),
            effective_perms: Some(ctx.permissions.clone()),
            connection_id: None,
        };
        dispatch(dispatcher.as_ref(), app, action, declaration.as_ref(), input, caller)
            .await
            .map_err(|e| e.to_string())
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
