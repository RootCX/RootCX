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

pub enum InboundEvent {
    Message { chat_id: String, text: String },
    Callback { chat_id: String, callback_id: String, data: String },
    Ignored,
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
    ) -> Result<InboundEvent, ChannelError>;

    async fn send_response(
        &self, config: &JsonValue, chat_id: &str, text: &str,
    ) -> Result<(), ChannelError>;

    async fn register_webhook(
        &self, config: &JsonValue, callback_url: &str,
    ) -> Result<(), ChannelError>;

    async fn unregister_webhook(&self, config: &JsonValue) -> Result<(), ChannelError>;

    async fn send_approval(
        &self, config: &JsonValue, chat_id: &str, _approval_id: &str,
        tool_name: &str, args: &JsonValue,
    ) -> Result<(), ChannelError> {
        self.send_response(config, chat_id, &format!(
            "⚙️ {tool_name}\n```\n{args}\n```\nApproval required — reply /approve or /deny",
        )).await
    }

    async fn answer_callback(&self, _config: &JsonValue, _callback_id: &str, _text: &str) -> Result<(), ChannelError> { Ok(()) }

    fn debounce_ms(&self) -> Option<u64> { None }
    fn start_typing(&self, _config: &JsonValue, _chat_id: &str) -> Option<tokio::task::AbortHandle> { None }
    async fn on_activate_boot(&self, _config: &JsonValue) {}
}
