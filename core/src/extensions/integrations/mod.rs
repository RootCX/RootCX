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
    mut params: serde_json::Value,
    identity: &crate::governance::enforcement::ContextState,
) -> Result<serde_json::Value, String> {
    use crate::tools::str_arg;
    // The requester is this worker's fixed identity — the only user an action
    // may act as (it can never name an arbitrary one). The unit run on its
    // behalf must carry a real RLS identity to the sub-worker, else it lands on
    // the anonymous worker and RLS hides every row.
    let requester = identity.user_id.ok_or("self_action requires an authenticated context")?;
    // Authority is monotone: a delegated worker propagates its frozen perms
    // down; a direct one acts with the user's own authority. Never re-widened.
    let inherit = identity.is_delegated.then(|| identity.effective_perms.as_slice());

    match action {
        "syncConnectedUsers" => {
            // Acting for ALL connected users is privileged. The old HTTP path
            // gated this on admin (connected-users + x-run-as both admin-only);
            // preserve that exactly via the shared require_admin helper.
            crate::governance::authority::require_admin(pool, requester)
                .await.map_err(|_| "syncConnectedUsers requires admin".to_string())?;
            let action_name = str_arg(&params, "actionName")?;
            let conns = connections::connected_connections(pool, app_id).await.map_err(|e| format!("{e:?}"))?;
            let mut synced = 0u64;
            for (uid, conn_id) in conns {
                // Each connection syncs with its own user's authority, pinned to
                // that specific connection so re-entries (ctx.action) resolve the
                // same mailbox. Disabled users skip.
                let Ok(user_uuid) = uid.parse::<uuid::Uuid>() else { continue };
                let Some(rpc) = crate::principal::resolve_caller_pinned(pool, user_uuid, conn_id).await else { continue };
                if caller.call(pool, user_uuid, None, app_id, action_name, serde_json::json!({}), Some(rpc)).await.is_ok() {
                    synced += 1;
                }
            }
            Ok(serde_json::json!({ "ok": true, "synced": synced }))
        }
        "triggerAction" => {
            // Scoped to the requester: runs the action for themselves only,
            // inheriting the caller's authority envelope AND connection pin.
            let action_name = str_arg(&params, "actionName")?;
            let input = params.get("input").cloned().unwrap_or_else(|| serde_json::json!({}));
            let mut rpc = crate::principal::resolve_caller_inheriting(pool, requester, inherit).await;
            if let Some(ref mut c) = rpc { c.connection_id = identity.connection_id.clone(); }
            caller.call(pool, requester, None, app_id, action_name, input, rpc).await
        }
        "call_integration" => {
            // A business-app worker calling ANOTHER integration's action.
            // The binding-as-consent rule (connections::binding_allows) gates it:
            // no token ever reaches the worker; the core executes with
            // credentials it owns, even from background jobs.
            let input = params.get_mut("input").map(serde_json::Value::take)
                .unwrap_or_else(|| serde_json::json!({}));
            let integration_id = str_arg(&params, "integrationId")?;
            let action_name = str_arg(&params, "action")?;
            let effective = match params.get("asUser").and_then(|v| v.as_str()) {
                Some(s) => s.parse::<uuid::Uuid>().map_err(|_| "call_integration: asUser must be a uuid".to_string())?,
                None => requester,
            };

            if !connections::binding_allows(pool, app_id, integration_id, requester, effective).await? {
                return Err(format!("no binding allows {app_id} to use {integration_id} as user {effective}"));
            }
            // Acting as oneself inherits the caller's envelope + connection pin;
            // acting as another user runs as that user's own authority (no pin —
            // the target integration resolves their connection normally).
            let mut rpc = if effective == requester {
                crate::principal::resolve_caller_inheriting(pool, requester, inherit).await
            } else {
                crate::principal::resolve_caller(pool, effective).await
            };
            if effective == requester {
                if let Some(ref mut c) = rpc { c.connection_id = identity.connection_id.clone(); }
            }
            caller.call(pool, effective, Some(app_id), integration_id, action_name, input, rpc).await
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
            .route("/api/v1/integrations/{integration_id}/configs",
                get(routes::list_configs).post(routes::create_config))
            .route("/api/v1/integrations/{integration_id}/configs/{config_id}",
                axum::routing::delete(routes::delete_config))
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
