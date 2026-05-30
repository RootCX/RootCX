pub(crate) mod auth;
mod catalog;
pub(crate) mod connections;
pub(crate) mod routes;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post, put};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;
use crate::tools::IntegrationCaller;

/// Privileged self-action dispatched over IPC by an integration worker (no
/// token replay). Replaces the old `syncAllConnectedUsers` HTTP callback that
/// required forwarding the user's JWT. The core runs the action with the
/// context it already owns.
pub async fn execute_self_action(
    pool: &PgPool,
    caller: &dyn IntegrationCaller,
    app_id: &str,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    match action {
        "syncConnectedUsers" => {
            let action_name = params.get("actionName").and_then(|v| v.as_str())
                .ok_or("syncConnectedUsers: missing actionName")?;
            let users = connections::connected_users(pool, app_id).await.map_err(|e| format!("{e:?}"))?;
            let mut synced = 0u64;
            for uid in users {
                if let Ok(user_uuid) = uid.parse::<uuid::Uuid>()
                    && caller.call(pool, user_uuid, app_id, action_name, serde_json::json!({})).await.is_ok()
                {
                    synced += 1;
                }
            }
            Ok(serde_json::json!({ "ok": true, "synced": synced }))
        }
        "triggerAction" => {
            let action_name = params.get("actionName").and_then(|v| v.as_str())
                .ok_or("triggerAction: missing actionName")?;
            let user_id = params.get("userId").and_then(|v| v.as_str())
                .and_then(|s| s.parse::<uuid::Uuid>().ok())
                .ok_or("triggerAction: missing/invalid userId")?;
            let input = params.get("input").cloned().unwrap_or_else(|| serde_json::json!({}));
            caller.call(pool, user_id, app_id, action_name, input).await
        }
        other => Err(format!("unknown self_action: {other}")),
    }
}

pub struct IntegrationsExtension;

#[async_trait]
impl RuntimeExtension for IntegrationsExtension {
    fn name(&self) -> &str { "integrations" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping integrations extension");
        for ddl in [
            "ALTER TABLE rootcx_system.apps ADD COLUMN IF NOT EXISTS webhook_token TEXT UNIQUE",
            "CREATE INDEX IF NOT EXISTS idx_apps_webhook_token
                ON rootcx_system.apps (webhook_token) WHERE webhook_token IS NOT NULL",
            "DO $$ BEGIN
                IF EXISTS (SELECT 1 FROM information_schema.tables WHERE table_schema = 'rootcx_system' AND table_name = 'integration_bindings') THEN
                    UPDATE rootcx_system.apps SET webhook_token = b.webhook_token
                        FROM rootcx_system.integration_bindings b
                        WHERE rootcx_system.apps.id = b.integration_id
                        AND rootcx_system.apps.webhook_token IS NULL
                        AND b.webhook_token IS NOT NULL;
                    DROP TABLE rootcx_system.integration_bindings;
                END IF;
            END $$",
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }
        connections::bootstrap(pool).await?;
        info!("integrations extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new()
            .route("/api/v1/integrations", get(routes::list_integrations))
            .route("/api/v1/integrations/catalog", get(catalog::list_catalog))
            .route("/api/v1/integrations/catalog/{id}/deploy", post(catalog::deploy_from_catalog))
            .route("/api/v1/integrations/catalog/{id}", axum::routing::delete(catalog::undeploy))
            .route("/api/v1/integrations/{integration_id}/config", put(routes::save_platform_config))
            .route("/api/v1/integrations/{integration_id}/actions/{action_id}", post(routes::execute_action))
            .route("/api/v1/integrations/{integration_id}/connected-users", get(routes::connected_users))
            .route("/api/v1/integrations/{integration_id}/auth", get(auth::status).delete(auth::disconnect))
            .route("/api/v1/integrations/{integration_id}/auth/start", post(auth::start))
            .route("/api/v1/integrations/{integration_id}/auth/credentials", post(auth::submit_credentials))
            .route("/api/v1/integrations/{integration_id}/auth/delegate", post(auth::delegate))
            .route("/api/v1/integrations/auth/callback", get(auth::callback))
            .route("/api/v1/hooks/{token}", post(routes::webhook_ingress))
            // Connections
            .route("/api/v1/integrations/{integration_id}/connections", get(connections::list_connections))
            .route("/api/v1/integrations/{integration_id}/connections/{connection_id}",
                axum::routing::patch(connections::update_connection).delete(connections::delete_connection))
            // App integration bindings
            .route("/api/v1/apps/{app_id}/integrations",
                get(connections::list_app_bindings).post(connections::bind_app))
            .route("/api/v1/apps/{app_id}/integrations/{integration_id}",
                axum::routing::delete(connections::unbind_app)))
    }
}
