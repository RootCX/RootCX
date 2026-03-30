use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub struct ChannelBinding {
    pub channel_id: String,
    pub app_id: String,
    pub routing: Option<JsonValue>,
}

pub struct InboundMessage {
    pub chat_id: String,
    pub text: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("invalid webhook: {0}")]
    InvalidWebhook(String),
    #[error("provider error: {0}")]
    Provider(String),
}

#[async_trait]
pub trait ChannelProvider: Send + Sync {
    async fn parse_webhook(
        &self, config: &JsonValue, body: Bytes, headers: &HeaderMap,
    ) -> Result<InboundMessage, ChannelError>;

    async fn send_response(
        &self, config: &JsonValue, chat_id: &str, text: &str,
    ) -> Result<(), ChannelError>;

    async fn register_webhook(
        &self, config: &JsonValue, callback_url: &str,
    ) -> Result<(), ChannelError>;

    async fn unregister_webhook(&self, config: &JsonValue) -> Result<(), ChannelError>;

    /// Providers that split long messages (e.g. Telegram at 4096 chars) return a
    /// debounce window. The webhook handler buffers messages per chat_id and
    /// concatenates them before invoking the agent.
    fn debounce_ms(&self) -> Option<u64> { None }
}
