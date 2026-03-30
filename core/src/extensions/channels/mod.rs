pub(crate) mod routes;
mod telegram;
pub(crate) mod types;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{delete, get, post};
use sqlx::PgPool;
use tracing::info;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;

pub struct ChannelExtension;

#[async_trait]
impl RuntimeExtension for ChannelExtension {
    fn name(&self) -> &str { "channels" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping channels extension");
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.channels (
                id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                provider    TEXT NOT NULL,
                name        TEXT NOT NULL,
                config      JSONB NOT NULL DEFAULT '{}',
                status      TEXT NOT NULL DEFAULT 'inactive',
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.channel_bindings (
                channel_id  UUID NOT NULL REFERENCES rootcx_system.channels(id) ON DELETE CASCADE,
                app_id      TEXT NOT NULL REFERENCES rootcx_system.agents(app_id) ON DELETE CASCADE,
                routing     JSONB,
                PRIMARY KEY (channel_id, app_id)
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.channel_sessions (
                channel_id       UUID NOT NULL REFERENCES rootcx_system.channels(id) ON DELETE CASCADE,
                external_chat_id TEXT NOT NULL,
                app_id           TEXT NOT NULL,
                session_id       UUID NOT NULL REFERENCES rootcx_system.agent_sessions(id) ON DELETE CASCADE,
                created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (channel_id, external_chat_id)
            )",
            "CREATE INDEX IF NOT EXISTS idx_channel_sessions_session
                ON rootcx_system.channel_sessions (session_id)",
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new()
            .route("/api/v1/channels", get(routes::list_channels).post(routes::create_channel))
            .route("/api/v1/channels/{channel_id}", delete(routes::delete_channel))
            .route("/api/v1/channels/{channel_id}/activate", post(routes::activate_channel))
            .route("/api/v1/channels/{channel_id}/deactivate", post(routes::deactivate_channel))
            .route("/api/v1/channels/{channel_id}/bindings", get(routes::list_bindings).post(routes::bind_agent))
            .route("/api/v1/channels/{channel_id}/bindings/{app_id}", delete(routes::unbind_agent))
            .route("/api/v1/channels/{provider}/{channel_id}/webhook", post(routes::webhook)))
    }
}

pub(crate) fn provider(name: &str) -> Option<Box<dyn types::ChannelProvider>> {
    match name {
        "telegram" => Some(Box::new(telegram::TelegramProvider::new())),
        _ => None,
    }
}
