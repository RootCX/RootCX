mod auth;
mod catalog;
pub(crate) mod routes;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post, put};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;

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
            .route("/api/v1/integrations/{integration_id}/auth", get(auth::status).delete(auth::disconnect))
            .route("/api/v1/integrations/{integration_id}/auth/start", post(auth::start))
            .route("/api/v1/integrations/{integration_id}/auth/credentials", post(auth::submit_credentials))
            .route("/api/v1/integrations/auth/callback", get(auth::callback))
            .route("/api/v1/hooks/{token}", post(routes::webhook_ingress)))
    }
}
