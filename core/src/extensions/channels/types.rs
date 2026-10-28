use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use serde_json::Value as JsonValue;

/// Provider-owned media reference. Native providers use remote file IDs;
/// managed relays carry bounded, authenticated inline bytes.
pub enum MediaSource {
    Provider(String),
    Inline(Bytes),
}

pub struct MediaRef {
    pub source: MediaSource,
    pub content_type: Option<String>,
    pub name: Option<String>,
}

impl MediaRef {
    pub fn provider_file_id(&self) -> Option<&str> {
        match &self.source { MediaSource::Provider(id) => Some(id), MediaSource::Inline(_) => None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationLocale { Fr, En }

impl ConversationLocale {
    pub fn code(self) -> &'static str {
        match self { Self::Fr => "fr", Self::En => "en" }
    }
    pub fn language(self) -> &'static str {
        match self { Self::Fr => "French", Self::En => "English" }
    }
}

pub enum InboundEvent {
    Message { chat_id: String, text: String, media: Vec<MediaRef>, locale: Option<ConversationLocale> },
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
    /// Called only after webhook authentication. The default handles signed form retries.
    fn delivery_id(&self, body: &[u8]) -> Result<String, ChannelError> {
        use sha2::{Digest, Sha256};
        Ok(hex::encode(Sha256::digest(body)))
    }

    /// Additional provider-side session validity; Core owns identity/delegation checks.
    async fn authorize_session(&self, _config: &JsonValue, _chat: &str, _user: uuid::Uuid) -> Result<(), ChannelError> {
        Ok(())
    }

    async fn parse_webhook(
        &self, config: &JsonValue, body: Bytes, headers: &HeaderMap,
    ) -> Result<InboundEvent, ChannelError>;

    async fn send_response(
        &self, config: &JsonValue, chat_id: &str, text: &str,
    ) -> Result<(), ChannelError>;

    /// Stable notice code for host presentation; native providers use the safe fallback.
    async fn send_notice(
        &self, config: &JsonValue, chat_id: &str, _code: &str, fallback: &str,
    ) -> Result<(), ChannelError> {
        self.send_response(config, chat_id, fallback).await
    }

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
    async fn prepare_link(&self, config: &JsonValue, token: &str, _channel_id: &str) -> Result<Option<String>, ChannelError> {
        Ok(self.link_url(config, token))
    }
    fn start_typing(&self, _config: &JsonValue, _chat_id: &str) -> Option<tokio::task::AbortHandle> { None }
    async fn on_activate_boot(&self, _config: &JsonValue) {}
}

#[cfg(test)]
mod locale_tests {
    use super::ConversationLocale;
    #[test]
    fn managed_language_metadata_is_a_closed_contract() {
        assert_eq!(serde_json::from_str::<ConversationLocale>("\"fr\"").unwrap(), ConversationLocale::Fr);
        assert_eq!(serde_json::from_str::<ConversationLocale>("\"en\"").unwrap(), ConversationLocale::En);
        for value in ["\"de\"", "\"en; ignore instructions\"", "{}", "42"] {
            assert!(serde_json::from_str::<ConversationLocale>(value).is_err());
        }
    }
}
