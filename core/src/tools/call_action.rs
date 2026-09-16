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
        let action_exists = actions.as_array()
            .map(|arr| arr.iter().any(|a| a.get("id").and_then(|v| v.as_str()) == Some(action)))
            .unwrap_or(false);

        if !action_exists {
            return Err(format!("action '{action}' not found in app '{app}'"));
        }

        let caller = ctx.action_caller.as_ref().ok_or("action calling unavailable")?;
        let effective_uid = ctx.invoker_user_id.unwrap_or(ctx.user_id);
        // Propagate the agent's pre-computed intersection so the target app's
        // RLS bounds the cross-app hop to grant(agent)∩perms(human) (Phase 6a).
        caller.call(app, action, input, effective_uid, &ctx.app_id, Some(ctx.permissions.clone())).await
    }
}
