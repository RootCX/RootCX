use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use tracing::warn;

use super::types::{ChannelError, ChannelProvider, InboundEvent, MediaRef};

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
struct Message {
    chat: Chat,
    text: Option<String>,
    caption: Option<String>,
    photo: Option<Vec<PhotoSize>>,
    audio: Option<TelegramFile>,
    voice: Option<TelegramFile>,
    document: Option<TelegramDocument>,
}

#[derive(Deserialize)]
struct PhotoSize {
    file_id: String,
    file_size: Option<i64>,
}

#[derive(Deserialize)]
struct TelegramFile {
    file_id: String,
    mime_type: Option<String>,
}

#[derive(Deserialize)]
struct TelegramDocument {
    file_id: String,
    file_name: Option<String>,
    mime_type: Option<String>,
}

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

        // Text: prefer message text, fall back to caption (media with text overlay)
        let text = msg.text.or(msg.caption).unwrap_or_default();

        // Collect media refs — no download here, just metadata. 200 OK must be fast.
        let mut media: Vec<MediaRef> = Vec::new();

        if let Some(photos) = msg.photo {
            // Telegram sends multiple resolutions; pick largest by file_size
            if let Some(best) = photos.into_iter().max_by_key(|p| p.file_size.unwrap_or(0)) {
                media.push(MediaRef {
                    provider_file_id: best.file_id,
                    content_type: Some("image/jpeg".into()),
                    name: Some("photo.jpg".into()),
                });
            }
        }

        if let Some(audio) = msg.audio {
            media.push(MediaRef {
                provider_file_id: audio.file_id,
                content_type: audio.mime_type.or(Some("audio/mpeg".into())),
                name: Some("audio".into()),
            });
        }

        if let Some(voice) = msg.voice {
            media.push(MediaRef {
                provider_file_id: voice.file_id,
                content_type: Some("audio/ogg".into()),
                name: Some("voice.ogg".into()),
            });
        }

        if let Some(doc) = msg.document {
            media.push(MediaRef {
                provider_file_id: doc.file_id,
                content_type: doc.mime_type,
                name: doc.file_name,
            });
        }

        // Stickers, polls, locations etc. have no text and no media — nothing for the agent to act on.
        if text.is_empty() && media.is_empty() {
            return Ok(InboundEvent::Ignored);
        }

        Ok(InboundEvent::Message { chat_id: msg.chat.id.to_string(), text, media })
    }

    async fn download_media(
        &self, config: &JsonValue, media_ref: &MediaRef,
    ) -> Option<(Bytes, String, String)> {
        let token = Self::token(config).ok()?;

        // Telegram files aren't directly URL-addressable; must call getFile first to resolve the path.
        let resp: JsonValue = self.http
            .get(Self::bot_url(token, &format!("getFile?file_id={}", media_ref.provider_file_id)))
            .send().await.ok()?
            .json().await.ok()?;
        let file_path = resp.pointer("/result/file_path")?.as_str()?;

        let url = format!("https://api.telegram.org/file/bot{token}/{file_path}");
        let bytes = self.http.get(&url).send().await.ok()?.bytes().await.ok()?;

        let content_type = media_ref.content_type.clone()
            .unwrap_or_else(|| "application/octet-stream".into());
        let name = media_ref.name.clone()
            .unwrap_or_else(|| "file".into());

        Some((bytes, content_type, name))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    async fn parse(config: serde_json::Value, headers: HeaderMap, body: serde_json::Value) -> Result<InboundEvent, ChannelError> {
        TelegramProvider::new().parse_webhook(
            &config,
            axum::body::Bytes::from(body.to_string()),
            &headers,
        ).await
    }

    #[tokio::test]
    async fn text_message_returns_message_event() {
        let event = parse(serde_json::json!({}), HeaderMap::new(), serde_json::json!({
            "message": { "chat": { "id": 42 }, "text": "hello" }
        })).await.unwrap();
        let InboundEvent::Message { chat_id, text, media } = event else { panic!("expected Message") };
        assert_eq!(chat_id, "42");
        assert_eq!(text, "hello");
        assert!(media.is_empty());
    }

    #[tokio::test]
    async fn caption_used_when_text_absent() {
        // Media with a caption: text is None, caption is the user's message.
        let event = parse(serde_json::json!({}), HeaderMap::new(), serde_json::json!({
            "message": { "chat": { "id": 1 }, "caption": "describe this image",
                "photo": [{ "file_id": "f1", "file_size": 1000 }] }
        })).await.unwrap();
        let InboundEvent::Message { text, media, .. } = event else { panic!("expected Message") };
        assert_eq!(text, "describe this image");
        assert_eq!(media.len(), 1);
    }

    #[tokio::test]
    async fn photo_without_caption_returns_message_not_ignored() {
        // Image with no text — agent must still receive it, not get Ignored.
        let event = parse(serde_json::json!({}), HeaderMap::new(), serde_json::json!({
            "message": { "chat": { "id": 1 }, "photo": [{ "file_id": "f1" }] }
        })).await.unwrap();
        let InboundEvent::Message { text, media, .. } = event else { panic!("expected Message") };
        assert_eq!(text, "");
        assert_eq!(media[0].provider_file_id, "f1");
    }

    #[tokio::test]
    async fn photo_largest_size_selected() {
        // Telegram sends multiple resolutions; we must pick the largest by file_size.
        let event = parse(serde_json::json!({}), HeaderMap::new(), serde_json::json!({
            "message": { "chat": { "id": 1 }, "photo": [
                { "file_id": "small", "file_size": 500   },
                { "file_id": "large", "file_size": 80000 },
                { "file_id": "mid",   "file_size": 5000  }
            ]}
        })).await.unwrap();
        let InboundEvent::Message { media, .. } = event else { panic!("expected Message") };
        assert_eq!(media[0].provider_file_id, "large");
    }

    #[tokio::test]
    async fn voice_and_document_extracted() {
        let event = parse(serde_json::json!({}), HeaderMap::new(), serde_json::json!({
            "message": { "chat": { "id": 5 },
                "voice": { "file_id": "v1", "mime_type": "audio/ogg" },
                "document": { "file_id": "d1", "file_name": "report.pdf", "mime_type": "application/pdf" } }
        })).await.unwrap();
        let InboundEvent::Message { media, .. } = event else { panic!("expected Message") };
        assert_eq!(media.len(), 2);
        let voice = media.iter().find(|m| m.provider_file_id == "v1").unwrap();
        assert_eq!(voice.content_type.as_deref(), Some("audio/ogg"));
        let doc = media.iter().find(|m| m.provider_file_id == "d1").unwrap();
        assert_eq!(doc.name.as_deref(), Some("report.pdf"));
    }

    #[tokio::test]
    async fn empty_message_no_text_no_media_is_ignored() {
        let event = parse(serde_json::json!({}), HeaderMap::new(), serde_json::json!({
            "message": { "chat": { "id": 1 } }
        })).await.unwrap();
        assert!(matches!(event, InboundEvent::Ignored));
    }

    #[tokio::test]
    async fn secret_token_mismatch_is_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-telegram-bot-api-secret-token", "wrong_secret".parse().unwrap());
        let result = parse(
            serde_json::json!({ "webhook_secret": "correct_secret" }),
            headers,
            serde_json::json!({}),
        ).await;
        assert!(matches!(result, Err(ChannelError::InvalidWebhook(_))));
    }

    #[tokio::test]
    async fn secret_token_match_passes() {
        let mut headers = HeaderMap::new();
        headers.insert("x-telegram-bot-api-secret-token", "mysecret".parse().unwrap());
        let result = parse(
            serde_json::json!({ "webhook_secret": "mysecret" }),
            headers,
            serde_json::json!({ "message": { "chat": { "id": 1 }, "text": "hi" } }),
        ).await;
        assert!(result.is_ok());
    }
}
