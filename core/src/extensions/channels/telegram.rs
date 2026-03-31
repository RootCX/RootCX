use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use tracing::warn;

use super::types::{ChannelError, ChannelProvider, InboundEvent};

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

    async fn sync_commands(&self, config: &JsonValue) {
        let Ok(token) = Self::token(config) else { return };
        let _ = self.http
            .post(Self::bot_url(token, "setMyCommands"))
            .json(&json!({ "commands": [
                { "command": "newsession", "description": "Start a new conversation" },
                { "command": "agent", "description": "Switch agent" },
            ]})).send().await;
    }

    async fn api_post(&self, config: &JsonValue, method: &str, body: &JsonValue) -> Result<(), ChannelError> {
        let resp = self.http
            .post(Self::bot_url(Self::token(config)?, method))
            .json(body).send().await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!(method, "telegram API failed: {body}");
            return Err(ChannelError::Provider(body));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct Update {
    message: Option<Message>,
    callback_query: Option<CallbackQuery>,
}
#[derive(Deserialize)]
struct Message { chat: Chat, text: Option<String> }
#[derive(Deserialize)]
struct CallbackQuery { id: String, from: ChatFrom, data: Option<String> }
#[derive(Deserialize)]
struct Chat { id: i64 }
#[derive(Deserialize)]
struct ChatFrom { id: i64 }

#[async_trait]
impl ChannelProvider for TelegramProvider {
    async fn parse_webhook(
        &self, config: &JsonValue, body: Bytes, headers: &HeaderMap,
    ) -> Result<InboundEvent, ChannelError> {
        if let Some(secret) = config["webhook_secret"].as_str() {
            let header = headers.get("x-telegram-bot-api-secret-token")
                .and_then(|v| v.to_str().ok()).unwrap_or("");
            if header != secret {
                return Err(ChannelError::InvalidWebhook("secret mismatch".into()));
            }
        }

        let update: Update = serde_json::from_slice(&body)
            .map_err(|e| ChannelError::InvalidWebhook(e.to_string()))?;

        if let Some(cb) = update.callback_query {
            return Ok(InboundEvent::Callback {
                chat_id: cb.from.id.to_string(),
                callback_id: cb.id,
                data: cb.data.unwrap_or_default(),
            });
        }

        let Some(msg) = update.message else { return Ok(InboundEvent::Ignored) };
        let Some(text) = msg.text else { return Ok(InboundEvent::Ignored) };
        Ok(InboundEvent::Message { chat_id: msg.chat.id.to_string(), text })
    }

    async fn send_response(
        &self, config: &JsonValue, chat_id: &str, text: &str,
    ) -> Result<(), ChannelError> {
        self.api_post(config, "sendMessage", &json!({
            "chat_id": chat_id, "text": text, "parse_mode": "Markdown",
        })).await
    }

    async fn send_approval(
        &self, config: &JsonValue, chat_id: &str, approval_id: &str,
        tool_name: &str, args: &JsonValue,
    ) -> Result<(), ChannelError> {
        self.api_post(config, "sendMessage", &json!({
            "chat_id": chat_id,
            "text": format!("⚙️ *{tool_name}*\n```\n{args}\n```"),
            "parse_mode": "Markdown",
            "reply_markup": { "inline_keyboard": [[
                { "text": "✅ Approve", "callback_data": format!("approve:{approval_id}") },
                { "text": "❌ Deny",    "callback_data": format!("deny:{approval_id}") },
            ]]}
        })).await
    }

    async fn send_choice(
        &self, config: &JsonValue, chat_id: &str, text: &str,
        options: &[(String, String)],
    ) -> Result<(), ChannelError> {
        let buttons: Vec<Vec<JsonValue>> = options.iter()
            .map(|(label, data)| vec![json!({ "text": label, "callback_data": data })])
            .collect();
        self.api_post(config, "sendMessage", &json!({
            "chat_id": chat_id, "text": text, "parse_mode": "Markdown",
            "reply_markup": { "inline_keyboard": buttons },
        })).await
    }

    async fn answer_callback(&self, config: &JsonValue, callback_id: &str, text: &str) -> Result<(), ChannelError> {
        self.api_post(config, "answerCallbackQuery", &json!({
            "callback_query_id": callback_id, "text": text,
        })).await
    }

    async fn register_webhook(
        &self, config: &JsonValue, callback_url: &str,
    ) -> Result<(), ChannelError> {
        let mut payload = json!({ "url": callback_url });
        if let Some(secret) = config["webhook_secret"].as_str() {
            payload["secret_token"] = json!(secret);
        }
        self.api_post(config, "setWebhook", &payload).await?;
        self.sync_commands(config).await;
        Ok(())
    }

    async fn unregister_webhook(&self, config: &JsonValue) -> Result<(), ChannelError> {
        let _ = self.api_post(config, "deleteWebhook", &json!({})).await;
        Ok(())
    }

    async fn resolve_bot_meta(&self, config: &JsonValue) -> Option<JsonValue> {
        let token = Self::token(config).ok()?;
        let resp = self.http.get(Self::bot_url(token, "getMe")).send().await.ok()?;
        let body: JsonValue = resp.json().await.ok()?;
        let username = body.pointer("/result/username")?.as_str()?;
        Some(json!({ "bot_username": username }))
    }

    fn link_url(&self, config: &JsonValue, token: &str) -> Option<String> {
        let username = config.get("bot_username").and_then(|v| v.as_str())?;
        Some(format!("https://t.me/{username}?start={token}"))
    }

    async fn on_activate_boot(&self, config: &JsonValue) { self.sync_commands(config).await; }

    fn debounce_ms(&self) -> Option<u64> { Some(2000) }

    fn start_typing(&self, config: &JsonValue, chat_id: &str) -> Option<tokio::task::AbortHandle> {
        let url = Self::bot_url(Self::token(config).ok()?, "sendChatAction");
        let body = json!({ "chat_id": chat_id, "action": "typing" });
        let http = self.http.clone();

        let handle = tokio::spawn(async move {
            loop {
                let _ = http.post(&url).json(&body).send().await;
                tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
            }
        });
        Some(handle.abort_handle())
    }
}
