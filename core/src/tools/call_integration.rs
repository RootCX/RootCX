use async_trait::async_trait;
use serde_json::{json, Value as JsonValue};
use rootcx_types::ToolDescriptor;

use super::{Tool, ToolContext, str_arg};

pub struct CallIntegrationTool;

#[async_trait]
impl Tool for CallIntegrationTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "call_integration".into(),
            description: "Execute an action on an installed integration. Use list_integrations first to discover available integrations and their actions/schemas.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "integration_id": { "type": "string", "description": "Integration identifier (e.g. gmail, linkedin, peppol)" },
                    "action": { "type": "string", "description": "Action ID to execute (e.g. send_email, whoami)" },
                    "input": { "type": "object", "description": "Action input matching the action's inputSchema", "default": {} }
                },
                "required": ["integration_id", "action"]
            }),
        }
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String> {
        let integration_id = str_arg(&ctx.args, "integration_id")?;
        let action = str_arg(&ctx.args, "action")?;
        let input = ctx.args.get("input").cloned().unwrap_or(json!({}));

        let caller = ctx.integration_caller.as_ref()
            .ok_or("integration calling unavailable")?;

        let effective_user = ctx.invoker_user_id.unwrap_or(ctx.user_id);

        // Same binding-as-consent rule as the worker path: an agent may only
        // drive an integration the app is bound to, and act as another user
        // only when that user has their own (app × user) binding.
        let allowed = crate::extensions::integrations::connections::binding_allows(
            &ctx.pool, &ctx.app_id, integration_id, ctx.user_id, effective_user,
        ).await?;
        if !allowed {
            return Err(format!("no binding allows {} to use {integration_id}", ctx.app_id));
        }

        // The integration runs under the human's RLS identity, but carrying the
        // agent's already-frozen (narrowed) authority — delegated, never
        // re-widened to the human's full set.
        let rpc = crate::principal::resolve_caller_inheriting(
            &ctx.pool, effective_user, Some(&ctx.permissions),
        ).await;
        caller.call(&ctx.pool, effective_user, Some(&ctx.app_id), integration_id, action, input, rpc).await
    }
}
