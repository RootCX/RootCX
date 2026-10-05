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

    fn webhook_secret(config: &JsonValue) -> Result<&str, ChannelError> {
        config["webhook_secret"].as_str().filter(|secret| !secret.is_empty())
            .ok_or_else(|| ChannelError::Provider("missing webhook_secret".into()))
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
            .map_err(|_| ChannelError::Provider("Telegram request unavailable".into()))?;
        let succeeded = resp.status().is_success();
        let body: JsonValue = resp.json().await.map_err(|_| ChannelError::Provider("invalid Telegram response".into()))?;
        if !succeeded || body["ok"] != true {
            warn!(method, "telegram API request failed");
            return Err(ChannelError::Provider("Telegram request failed".into()));
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
    from: Option<ChatFrom>,
    sender_chat: Option<Chat>,
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
struct CallbackQuery { id: String, from: ChatFrom, message: Option<CallbackMessage>, data: Option<String> }
#[derive(Deserialize)]
struct CallbackMessage { chat: Chat }
#[derive(Deserialize)]
struct Chat { id: i64 }
#[derive(Deserialize)]
struct ChatFrom { id: i64, is_bot: bool }

fn bound_chat(chat: i64, user: i64) -> Result<String, ChannelError> {
    if chat == 0 || user <= 0 { return Err(ChannelError::InvalidWebhook("invalid chat or sender".into())); }
    Ok(format!("telegram:{}", json!([chat, user])))
}

fn delivery_chat(identity: &str) -> Result<i64, ChannelError> {
    let pair = identity.strip_prefix("telegram:").and_then(|value| serde_json::from_str::<(i64, i64)>(value).ok());
    match pair {
        Some((chat, user)) if chat != 0 && user > 0 => Ok(chat),
        _ => Err(ChannelError::Provider("channel identity must be relinked".into())),
    }
}

#[async_trait]
impl ChannelProvider for TelegramProvider {
    fn delivery_id(&self, body: &[u8]) -> Result<String, ChannelError> {
        let value: JsonValue = serde_json::from_slice(body).map_err(|_| ChannelError::InvalidWebhook("invalid update".into()))?;
        value["update_id"].as_u64().map(|id| id.to_string())
            .ok_or_else(|| ChannelError::InvalidWebhook("missing update_id".into()))
    }

    async fn parse_webhook(
        &self, config: &JsonValue, body: Bytes, headers: &HeaderMap,
    ) -> Result<InboundEvent, ChannelError> {
        let secret = Self::webhook_secret(config)?;
        let header = headers.get("x-telegram-bot-api-secret-token")
            .and_then(|v| v.to_str().ok()).unwrap_or("");
        if header != secret {
            return Err(ChannelError::InvalidWebhook("secret mismatch".into()));
        }

        let update: Update = serde_json::from_slice(&body)
            .map_err(|e| ChannelError::InvalidWebhook(e.to_string()))?;

        if let Some(cb) = update.callback_query {
            if cb.from.is_bot { return Ok(InboundEvent::Ignored); }
            let message = cb.message.ok_or_else(|| ChannelError::InvalidWebhook("callback chat missing".into()))?;
            return Ok(InboundEvent::Callback {
                chat_id: bound_chat(message.chat.id, cb.from.id)?,
                callback_id: cb.id,
                data: cb.data.unwrap_or_default(),
            });
        }

        let Some(msg) = update.message else { return Ok(InboundEvent::Ignored) };
        if msg.sender_chat.is_some() { return Ok(InboundEvent::Ignored); }
        let sender = msg.from.ok_or_else(|| ChannelError::InvalidWebhook("missing sender".into()))?;
        if sender.is_bot { return Ok(InboundEvent::Ignored); }
        let chat_id = bound_chat(msg.chat.id, sender.id)?;

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

        Ok(InboundEvent::Message { chat_id, text, media })
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
        // Plain text accepts arbitrary model output; split by characters, never UTF-8 bytes.
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(4096) {
            self.api_post(config, "sendMessage", &json!({
                "chat_id": delivery_chat(chat_id)?, "text": chunk.iter().collect::<String>(),
            })).await?;
        }
        Ok(())
    }

    async fn send_approval(
        &self, config: &JsonValue, chat_id: &str, approval_id: &str,
        tool_name: &str, args: &JsonValue,
    ) -> Result<(), ChannelError> {
        self.api_post(config, "sendMessage", &json!({
            "chat_id": delivery_chat(chat_id)?,
            "text": format!("Confirm {tool_name}?\n{args}"),
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
            "chat_id": delivery_chat(chat_id)?, "text": text,
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
        let payload = json!({ "url": callback_url, "secret_token": Self::webhook_secret(config)? });
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


    fn start_typing(&self, config: &JsonValue, chat_id: &str) -> Option<tokio::task::AbortHandle> {
        let url = Self::bot_url(Self::token(config).ok()?, "sendChatAction");
        let body = json!({ "chat_id": delivery_chat(chat_id).ok()?, "action": "typing" });
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

    async fn authenticated(body: JsonValue) -> Result<InboundEvent, ChannelError> {
        let mut headers = HeaderMap::new();
        headers.insert("x-telegram-bot-api-secret-token", "test-secret".parse().unwrap());
        parse(json!({"webhook_secret": "test-secret"}), headers, body).await
    }

    #[tokio::test]
    async fn group_members_have_distinct_identities_matching_their_callbacks() {
        let mut identities = Vec::new();
        for user in [41, 42] {
            let event = authenticated(json!({"message": {
                "chat":{"id":-100}, "from":{"id":user,"is_bot":false}, "text":"/link link_token"
            }})).await.unwrap();
            let InboundEvent::Message { chat_id, .. } = event else { panic!("expected message"); };
            let event = authenticated(json!({"callback_query":{
                "id":"callback", "from":{"id":user,"is_bot":false},
                "message":{"chat":{"id":-100}}, "data":"approve:pending"
            }})).await.unwrap();
            let InboundEvent::Callback { chat_id: replied, .. } = event else { panic!("expected callback"); };
            assert_eq!(chat_id, replied, "user={user}");
            assert_eq!(delivery_chat(&chat_id).unwrap(), -100, "user={user}");
            identities.push(chat_id);
        }
        assert_ne!(identities[0], identities[1], "a shared group cannot share a Core identity");
    }

    #[tokio::test]
    async fn missing_actor_or_callback_room_is_rejected() {
        for body in [
            json!({"message":{"chat":{"id":42},"text":"/link link_token"}}),
            json!({"callback_query":{"id":"a","message":{"chat":{"id":42}},"data":"approve:a"}}),
            json!({"callback_query":{"id":"a","from":{"id":42,"is_bot":false},"data":"approve:a"}}),
        ] {
            assert!(authenticated(body.clone()).await.is_err(), "{body}");
        }
    }

    #[tokio::test]
    async fn bot_and_anonymous_room_senders_are_ignored() {
        for body in [
            json!({"message":{"chat":{"id":42},"from":{"id":1,"is_bot":true},"text":"hello"}}),
            json!({"message":{"chat":{"id":-100},"sender_chat":{"id":-100},"text":"hello"}}),
            json!({"callback_query":{"id":"a","from":{"id":1,"is_bot":true},"message":{"chat":{"id":42}},"data":"approve:a"}}),
        ] {
            assert!(matches!(authenticated(body.clone()).await.unwrap(), InboundEvent::Ignored), "{body}");
        }
    }

    #[tokio::test]
    async fn webhook_authentication_cannot_be_omitted() {
        for (config, secret) in [(json!({}), Some("s")), (json!({"webhook_secret":""}), None), (json!({"webhook_secret":"s"}), None)] {
            let mut headers = HeaderMap::new();
            if let Some(secret) = secret { headers.insert("x-telegram-bot-api-secret-token", secret.parse().unwrap()); }
            assert!(parse(config.clone(), headers, json!({"message":{
                "chat":{"id":42},"from":{"id":42,"is_bot":false},"text":"/link link_token"
            }})).await.is_err(), "config={config}, secret={secret:?}");
        }
    }

    #[test]
    fn delivery_never_falls_back_to_legacy_or_incomplete_identities() {
        for identity in ["42", "-100", "telegram:42", "telegram:[42]", "telegram:[42,0]", "telegram:[0,42]", "slack:[\"C42\",\"U1\"]"] {
            assert!(delivery_chat(identity).is_err(), "{identity}");
        }
    }

    #[tokio::test]
    async fn text_message_returns_message_event() {
        let event = authenticated(serde_json::json!({
            "message": { "from": { "id": 42, "is_bot": false }, "chat": { "id": 42 }, "text": "hello" }
        })).await.unwrap();
        let InboundEvent::Message { chat_id, text, media } = event else { panic!("expected Message") };
        assert_eq!(chat_id, "telegram:[42,42]");
        assert_eq!(text, "hello");
        assert!(media.is_empty());
    }

    #[tokio::test]
    async fn caption_used_when_text_absent() {
        // Media with a caption: text is None, caption is the user's message.
        let event = authenticated(serde_json::json!({
            "message": { "from": { "id": 42, "is_bot": false }, "chat": { "id": 1 }, "caption": "describe this image",
                "photo": [{ "file_id": "f1", "file_size": 1000 }] }
        })).await.unwrap();
        let InboundEvent::Message { text, media, .. } = event else { panic!("expected Message") };
        assert_eq!(text, "describe this image");
        assert_eq!(media.len(), 1);
    }

    #[tokio::test]
    async fn photo_without_caption_returns_message_not_ignored() {
        // Image with no text — agent must still receive it, not get Ignored.
        let event = authenticated(serde_json::json!({
            "message": { "from": { "id": 42, "is_bot": false }, "chat": { "id": 1 }, "photo": [{ "file_id": "f1" }] }
        })).await.unwrap();
        let InboundEvent::Message { text, media, .. } = event else { panic!("expected Message") };
        assert_eq!(text, "");
        assert_eq!(media[0].provider_file_id, "f1");
    }

    #[tokio::test]
    async fn photo_largest_size_selected() {
        // Telegram sends multiple resolutions; we must pick the largest by file_size.
        let event = authenticated(serde_json::json!({
            "message": { "from": { "id": 42, "is_bot": false }, "chat": { "id": 1 }, "photo": [
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
        let event = authenticated(serde_json::json!({
            "message": { "from": { "id": 42, "is_bot": false }, "chat": { "id": 5 },
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
        let event = authenticated(serde_json::json!({
            "message": { "from": { "id": 42, "is_bot": false }, "chat": { "id": 1 } }
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
            serde_json::json!({ "message": { "from": { "id": 42, "is_bot": false }, "chat": { "id": 1 }, "text": "hi" } }),
        ).await;
        assert!(result.is_ok());
    }
}
