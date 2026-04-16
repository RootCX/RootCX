use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use serde_json::Value as JsonValue;

/// Provider-agnostic reference to a media file from an inbound message.
/// Contains only metadata — bytes are fetched later via `download_media`.
pub struct MediaRef {
    pub provider_file_id: String,
    pub content_type: Option<String>,
    pub name: Option<String>,
}

pub enum InboundEvent {
    Message { chat_id: String, text: String, media: Vec<MediaRef> },
    Callback { chat_id: String, callback_id: String, data: String },
    /// Synchronous reply body (e.g. Slack URL verification challenge or slash command ack).
    /// Bypasses the active-channel check and is returned directly as HTTP response.
    Reply(JsonValue),
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

    /// Download a media file referenced in an inbound message.
    /// Returns (bytes, content_type, name) or None if not supported / failed.
    async fn download_media(
        &self, _config: &JsonValue, _media_ref: &MediaRef,
    ) -> Option<(Bytes, String, String)> { None }

    async fn send_approval(
        &self, config: &JsonValue, chat_id: &str, _approval_id: &str,
        tool_name: &str, args: &JsonValue,
    ) -> Result<(), ChannelError> {
        self.send_response(config, chat_id, &format!(
            "⚙️ {tool_name}\n```\n{args}\n```\nApproval required — reply /approve or /deny",
        )).await
    }

    async fn send_choice(
        &self, config: &JsonValue, chat_id: &str, text: &str,
        options: &[(String, String)],
    ) -> Result<(), ChannelError> {
        let list = options.iter().map(|(l, _)| format!("• {l}")).collect::<Vec<_>>().join("\n");
        self.send_response(config, chat_id, &format!("{text}\n{list}")).await
    }

    async fn answer_callback(&self, _config: &JsonValue, _callback_id: &str, _text: &str) -> Result<(), ChannelError> { Ok(()) }

    async fn resolve_bot_meta(&self, _config: &JsonValue) -> Option<JsonValue> { None }
    fn link_url(&self, _config: &JsonValue, _token: &str) -> Option<String> { None }
    fn debounce_ms(&self) -> Option<u64> { None }
    fn start_typing(&self, _config: &JsonValue, _chat_id: &str) -> Option<tokio::task::AbortHandle> { None }
    async fn on_activate_boot(&self, _config: &JsonValue) {}
}
