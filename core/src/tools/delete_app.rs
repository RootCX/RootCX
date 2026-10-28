use super::{Tool, ToolConfirmation, ToolContext, check_permission, str_arg};
use crate::governance::{app_deletion, execution::AgentAuthority};
use async_trait::async_trait;
use rootcx_types::ToolDescriptor;
use serde_json::{Value, json};

pub struct DeleteAppTool;

#[async_trait]
impl Tool for DeleteAppTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "delete_app".into(),
            description: "Permanently delete an installed application, its data, files and source and change history for all users. Core shows one confirmation to the responsible person; do not ask for another confirmation yourself. Returns a durable deletion operation; only state succeeded means deletion is complete. Use get_app_deletion to inspect progress. Cleanup automatically resumes after interruption. Cannot delete agents or integrations.".into(),
            input_schema: json!({"type":"object","properties":{"appId":{"type":"string"}},"required":["appId"],"additionalProperties":false}),
        }
    }
    fn requires_agent_context(&self) -> bool {
        true
    }

    async fn prepare_confirmation(
        &self,
        pool: &sqlx::PgPool,
        authority: &AgentAuthority,
        args: &Value,
    ) -> Result<Option<ToolConfirmation>, String> {
        let permissions = authority.revalidate(pool).await?;
        check_permission(&permissions, "tool:delete_app")?;
        check_permission(&permissions, app_deletion::PERMISSION)?;
        let operation = app_deletion::prepare(
            pool,
            str_arg(args, "appId")?,
            authority.human(),
            Some(authority.app()),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        Ok(Some(ToolConfirmation {
            args: json!({"appId": operation.app_id, "applicationName": operation.application_name, "operationId": operation.id}),
            reason: format!(
                "Permanently delete “{}”, including its data, files, source code and change history for all users?",
                operation.application_name
            ),
        }))
    }

    async fn execute(&self, ctx: &ToolContext) -> Result<Value, String> {
        let authority = ctx
            .delegated_authority()
            .ok_or("deletion requires live agent authority")?;
        if !authority.confirmed_call("delete_app", &ctx.args) {
            return Err("deletion requires the responsible person's confirmation".into());
        }
        let id = str_arg(&ctx.args, "operationId")?
            .parse()
            .map_err(|_| "invalid deletion operation")?;
        let operation = app_deletion::confirm(&ctx.pool, id, authority.human(), Some(authority))
            .await
            .map_err(|error| format!("{error:?}"))?;
        serde_json::to_value(operation).map_err(|error| error.to_string())
    }
}

pub struct AppDeletionStatusTool;

#[async_trait]
impl Tool for AppDeletionStatusTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "get_app_deletion".into(),
            description: "Inspect a deletion operation returned by delete_app. Only succeeded means the app has been removed; deleting or retry means Core is still completing cleanup. Never ask to confirm an already admitted operation again.".into(),
            input_schema: json!({"type":"object","properties":{"operationId":{"type":"string"}},"required":["operationId"],"additionalProperties":false}),
        }
    }
    async fn execute(&self, ctx: &ToolContext) -> Result<Value, String> {
        check_permission(&ctx.permissions, app_deletion::PERMISSION)?;
        let human = ctx.data_user_id().ok_or("responsible human missing")?;
        app_deletion::authorize(&ctx.pool, human)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let id = str_arg(&ctx.args, "operationId")?
            .parse()
            .map_err(|_| "invalid deletion operation")?;
        let operation = app_deletion::get(&ctx.pool, id)
            .await
            .map_err(|error| format!("{error:?}"))?;
        serde_json::to_value(operation).map_err(|error| error.to_string())
    }
}
