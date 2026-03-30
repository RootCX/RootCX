use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use tracing::warn;

use super::types::{ChannelError, ChannelProvider, InboundMessage};

pub struct TelegramProvider {
    http: reqwest::Client,
}

impl TelegramProvider {
    pub fn new() -> Self { Self { http: reqwest::Client::new() } }

    fn bot_url(token: &str, method: &str) -> String {
        format!("https://api.telegram.org/bot{token}/{method}")
    }

    fn token(config: &JsonValue) -> Result<&str, ChannelError> {
        config["bot_token"].as_str()
            .ok_or_else(|| ChannelError::Provider("missing bot_token".into()))
    }
}

#[derive(Deserialize)]
struct Update { message: Option<Message> }
#[derive(Deserialize)]
struct Message { chat: Chat, text: Option<String> }
#[derive(Deserialize)]
struct Chat { id: i64 }

#[async_trait]
impl ChannelProvider for TelegramProvider {
    async fn parse_webhook(
        &self, config: &JsonValue, body: Bytes, headers: &HeaderMap,
    ) -> Result<InboundMessage, ChannelError> {
        if let Some(secret) = config["webhook_secret"].as_str() {
            let header = headers.get("x-telegram-bot-api-secret-token")
                .and_then(|v| v.to_str().ok()).unwrap_or("");
            if header != secret {
                return Err(ChannelError::InvalidWebhook("secret mismatch".into()));
            }
        }

        let update: Update = serde_json::from_slice(&body)
            .map_err(|e| ChannelError::InvalidWebhook(e.to_string()))?;
        let msg = update.message
            .ok_or_else(|| ChannelError::InvalidWebhook("no message".into()))?;
        let text = msg.text
            .ok_or_else(|| ChannelError::InvalidWebhook("no text".into()))?;

        Ok(InboundMessage { chat_id: msg.chat.id.to_string(), text })
    }

    async fn send_response(
        &self, config: &JsonValue, chat_id: &str, text: &str,
    ) -> Result<(), ChannelError> {
        let resp = self.http
            .post(Self::bot_url(Self::token(config)?, "sendMessage"))
            .json(&json!({ "chat_id": chat_id, "text": text, "parse_mode": "Markdown" }))
            .send().await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(chat_id, "telegram sendMessage failed: {body}");
            return Err(ChannelError::Provider(body));
        }
        Ok(())
    }

    async fn register_webhook(
        &self, config: &JsonValue, callback_url: &str,
    ) -> Result<(), ChannelError> {
        let mut payload = json!({ "url": callback_url });
        if let Some(secret) = config["webhook_secret"].as_str() {
            payload["secret_token"] = json!(secret);
        }

        let resp = self.http
            .post(Self::bot_url(Self::token(config)?, "setWebhook"))
            .json(&payload).send().await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::Provider(format!("setWebhook failed: {body}")));
        }
        Ok(())
    }

    async fn unregister_webhook(&self, config: &JsonValue) -> Result<(), ChannelError> {
        let _ = self.http
            .post(Self::bot_url(Self::token(config)?, "deleteWebhook"))
            .send().await;
        Ok(())
    }

    fn debounce_ms(&self) -> Option<u64> { Some(2000) }
}
