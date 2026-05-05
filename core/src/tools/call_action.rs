use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg, check_permission};

pub struct CallActionTool;

#[async_trait]
impl Tool for CallActionTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "call_action".into(),
            description: "Execute an action exposed by an app. Use list_actions to discover available actions and their input schemas.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "app": { "type": "string", "description": "The app ID that exposes the action" },
                    "action": { "type": "string", "description": "The action ID to execute" },
                    "input": { "type": "object", "description": "Action input matching the action's inputSchema", "default": {} }
                },
                "required": ["app", "action"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let app = str_arg(&ctx.args, "app")?;
        let action = str_arg(&ctx.args, "action")?;
        let input = ctx.args.get("input").cloned().unwrap_or(json!({}));

        check_permission(&ctx.permissions, &format!("app:{app}:action:{action}"))?;

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
        caller.call(app, action, input, effective_uid).await
    }
}
