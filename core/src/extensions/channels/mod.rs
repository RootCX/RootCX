pub(crate) mod routes;
mod slack;
mod telegram;
pub(crate) mod types;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{delete, get, post};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use super::RuntimeExtension;
use crate::RuntimeError;
use crate::routes::SharedRuntime;

/// Namespace for channel delegation trigger_ref derivation.
/// Same pattern as AGENT_UUID_NAMESPACE in agents/mod.rs.
const CHANNEL_UUID_NAMESPACE: Uuid = Uuid::from_bytes([
    0xb1, 0x2c, 0x3d, 0x4e, 0x5f, 0x60, 0x41, 0x72,
    0x83, 0x94, 0xa5, 0xb6, 0xc7, 0xd8, 0xe9, 0xfa,
]);

/// Deterministic trigger_ref for a channel delegation, derived from (channel_id, user_id).
/// Enables revoke_by_trigger("channel", ref) when the link is removed.
pub fn channel_delegation_ref(channel_id: &str, user_id: Uuid) -> Uuid {
    let key = format!("{}:{user_id}", channel_id.to_ascii_lowercase());
    Uuid::new_v5(&CHANNEL_UUID_NAMESPACE, key.as_bytes())
}

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
            "DROP TABLE IF EXISTS rootcx_system.channel_bindings",
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
            "CREATE TABLE IF NOT EXISTS rootcx_system.channel_identities (
                channel_id       UUID NOT NULL REFERENCES rootcx_system.channels(id) ON DELETE CASCADE,
                external_chat_id TEXT NOT NULL,
                user_id          UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                linked_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (channel_id, external_chat_id)
            )",
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }

        let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT provider, config FROM rootcx_system.channels WHERE status = 'active'",
        ).fetch_all(pool).await.unwrap_or_default();
        for (prov, cfg) in rows {
            if let Some(p) = provider(&prov) { p.on_activate_boot(&cfg).await; }
        }

        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new()
            .route("/api/v1/channels", get(routes::list_channels).post(routes::create_channel))
            .route("/api/v1/channels/{channel_id}", delete(routes::delete_channel))
            .route("/api/v1/channels/{channel_id}/activate", post(routes::activate_channel))
            .route("/api/v1/channels/{channel_id}/deactivate", post(routes::deactivate_channel))
            .route("/api/v1/channels/{channel_id}/link", post(routes::create_link_token))
            .route("/api/v1/channels/{channel_id}/identity", get(routes::identity_status).delete(routes::unlink_identity))
            .route("/api/v1/channels/{provider}/{channel_id}/webhook", post(routes::webhook)))
    }
}

pub(crate) fn provider(name: &str) -> Option<Box<dyn types::ChannelProvider>> {
    match name {
        "telegram" => Some(Box::new(telegram::TelegramProvider::new())),
        "slack" => Some(Box::new(slack::SlackProvider::new())),
        _ => None,
    }
}
