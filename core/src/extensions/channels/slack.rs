use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use axum::body::Bytes;
use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use sha2::Sha256;
use tracing::warn;

use super::types::{ChannelError, ChannelProvider, InboundEvent, MediaRef};

type HmacSha256 = Hmac<Sha256>;

const SLACK_API: &str = "https://slack.com/api";
const TS_TOLERANCE_SECS: i64 = 300;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

pub struct SlackProvider;

impl SlackProvider {
    pub fn new() -> Self { Self }

    fn token(config: &JsonValue) -> Result<&str, ChannelError> {
        config["bot_token"].as_str()
            .ok_or_else(|| ChannelError::Provider("missing bot_token".into()))
    }

    fn signing_secret(config: &JsonValue) -> Result<&str, ChannelError> {
        config["signing_secret"].as_str()
            .filter(|secret| !secret.is_empty())
            .ok_or_else(|| ChannelError::Provider("missing signing_secret".into()))
    }

    fn verify_signature(config: &JsonValue, body: &[u8], headers: &HeaderMap) -> Result<(), ChannelError> {
        let secret = Self::signing_secret(config)?;
        let ts = headers.get("x-slack-request-timestamp")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ChannelError::InvalidWebhook("missing timestamp".into()))?;
        let sig = headers.get("x-slack-signature")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ChannelError::InvalidWebhook("missing signature".into()))?;

        let ts_int: i64 = ts.parse()
            .map_err(|_| ChannelError::InvalidWebhook("bad timestamp".into()))?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64).unwrap_or(0);
        if (now - ts_int).abs() > TS_TOLERANCE_SECS {
            return Err(ChannelError::InvalidWebhook("stale timestamp".into()));
        }

        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .map_err(|e| ChannelError::Provider(e.to_string()))?;
        mac.update(b"v0:");
        mac.update(ts.as_bytes());
        mac.update(b":");
        mac.update(body);
        let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

        if expected.len() != sig.len()
            || expected.as_bytes().iter().zip(sig.as_bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0
        {
            return Err(ChannelError::InvalidWebhook("signature mismatch".into()));
        }
        Ok(())
    }

    async fn api_post(&self, config: &JsonValue, method: &str, body: &JsonValue) -> Result<JsonValue, ChannelError> {
        let resp = HTTP.post(format!("{SLACK_API}/{method}"))
            .bearer_auth(Self::token(config)?)
            .json(body).send().await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;
        let json: JsonValue = resp.json().await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;
        if !json["ok"].as_bool().unwrap_or(false) {
            let err = json["error"].as_str().unwrap_or("unknown").to_string();
            warn!(method, "slack API failed: {err}");
            return Err(ChannelError::Provider(err));
        }
        Ok(json)
    }
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Envelope {
    #[serde(rename = "url_verification")]
    UrlVerification { challenge: String },
    #[serde(rename = "event_callback")]
    EventCallback { event: Event },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Event {
    #[serde(rename = "message")]
    Message {
        channel: String,
        user: Option<String>,
        channel_type: Option<String>,
        text: Option<String>,
        bot_id: Option<String>,
        subtype: Option<String>,
        files: Option<Vec<File>>,
    },
    #[serde(rename = "app_mention")]
    AppMention {
        channel: String,
        user: Option<String>,
        bot_id: Option<String>,
        text: Option<String>,
        files: Option<Vec<File>>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct File {
    id: String,
    name: Option<String>,
    mimetype: Option<String>,
}

#[derive(Deserialize)]
struct Interactive {
    actions: Vec<Action>,
    user: User,
    channel: Option<Channel>,
    response_url: Option<String>,
}

#[derive(Deserialize)]
struct Action { value: String }

#[derive(Deserialize)]
struct User { id: String }

#[derive(Deserialize)]
struct Channel { id: String }

// A room is a delivery address, never the identity of the person invoking Core.
fn bound_chat(channel: &str, user: Option<&str>) -> Result<String, ChannelError> {
    let user = user.filter(|id| !id.is_empty())
        .ok_or_else(|| ChannelError::InvalidWebhook("missing sender".into()))?;
    if channel.is_empty() { return Err(ChannelError::InvalidWebhook("missing channel".into())); }
    Ok(format!("slack:{}", json!([channel, user])))
}

fn delivery_chat(identity: &str) -> Result<String, ChannelError> {
    let pair = identity.strip_prefix("slack:").and_then(|value| serde_json::from_str::<(String, String)>(value).ok());
    match pair {
        Some((channel, user)) if !channel.is_empty() && !user.is_empty() => Ok(channel),
        _ => Err(ChannelError::Provider("channel identity must be relinked".into())),
    }
}

fn validate_response_url(url: Option<String>) -> Result<String, ChannelError> {
    let url = url.unwrap_or_default();
    if url.is_empty() { return Ok(url); }
    if url.starts_with("https://hooks.slack.com/") {
        Ok(url)
    } else {
        Err(ChannelError::InvalidWebhook("invalid response_url host".into()))
    }
}

fn extract_media(files: Option<Vec<File>>) -> Vec<MediaRef> {
    files.unwrap_or_default().into_iter().map(|f| MediaRef {
        provider_file_id: f.id,
        content_type: f.mimetype,
        name: f.name,
    }).collect()
}

#[async_trait]
impl ChannelProvider for SlackProvider {
    fn delivery_id(&self, body: &[u8]) -> Result<String, ChannelError> {
        use sha2::{Digest, Sha256};
        let event = serde_json::from_slice::<JsonValue>(body).ok();
        Ok(event.as_ref().and_then(|e| e["event_id"].as_str()).map(str::to_owned)
            .unwrap_or_else(|| hex::encode(Sha256::digest(body))))
    }

    async fn parse_webhook(
        &self, config: &JsonValue, body: Bytes, headers: &HeaderMap,
    ) -> Result<InboundEvent, ChannelError> {
        // url_verification is a one-time setup handshake. Slack sends it before
        // the user has copied the signing_secret into our config, so we cannot
        // verify the signature yet. Echoing the challenge is harmless: it only
        // proves URL liveness, not ownership of any account.
        if let Ok(envelope) = serde_json::from_slice::<Envelope>(&body) {
            if let Envelope::UrlVerification { challenge } = envelope {
                return Ok(InboundEvent::Reply(json!({ "challenge": challenge })));
            }
        }

        Self::verify_signature(config, &body, headers)?;

        // Form-encoded: slash commands and interactive payloads (Block Kit buttons).
        let content_type = headers.get("content-type")
            .and_then(|v| v.to_str().ok()).unwrap_or("application/json");
        if content_type.starts_with("application/x-www-form-urlencoded") {
            let form: HashMap<String, String> = url::form_urlencoded::parse(&body)
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();

            // Slash command: /link <token>
            if let Some(cmd) = form.get("command") {
                if cmd == "/link" {
                    let token = form.get("text").map(|s| s.trim().to_string()).unwrap_or_default();
                    let chat_id = bound_chat(form.get("channel_id").map(String::as_str).unwrap_or(""), form.get("user_id").map(String::as_str))?;
                    return Ok(InboundEvent::Message {
                        chat_id,
                        text: format!("/link {token}"),
                        media: vec![],
                    });
                }
                return Ok(InboundEvent::Ignored);
            }

            // Interactive payload (Block Kit buttons)
            let payload_str = form.get("payload")
                .ok_or_else(|| ChannelError::InvalidWebhook("missing payload".into()))?;
            let payload: Interactive = serde_json::from_str(payload_str)
                .map_err(|e| ChannelError::InvalidWebhook(e.to_string()))?;
            let action = payload.actions.into_iter().next()
                .ok_or_else(|| ChannelError::InvalidWebhook("no actions".into()))?;
            let chat_id = bound_chat(payload.channel.as_ref().map(|c| c.id.as_str()).unwrap_or(""), Some(&payload.user.id))?;
            let callback_id = validate_response_url(payload.response_url)?;
            return Ok(InboundEvent::Callback { chat_id, callback_id, data: action.value });
        }

        let envelope: Envelope = serde_json::from_slice(&body)
            .map_err(|e| ChannelError::InvalidWebhook(e.to_string()))?;

        match envelope {
            Envelope::UrlVerification { .. } => Ok(InboundEvent::Ignored),
            Envelope::EventCallback { event } => match event {
                Event::Message { channel, user, channel_type, text, bot_id, subtype, files } => {
                    // Skip bot's own messages, edits, deletions
                    if bot_id.is_some() || subtype.is_some() { return Ok(InboundEvent::Ignored); }
                    // v1: DMs only
                    if channel_type.as_deref() != Some("im") { return Ok(InboundEvent::Ignored); }
                    let media = extract_media(files);
                    let text = text.unwrap_or_default();
                    if text.is_empty() && media.is_empty() {
                        return Ok(InboundEvent::Ignored);
                    }
                    Ok(InboundEvent::Message { chat_id: bound_chat(&channel, user.as_deref())?, text, media })
                }
                Event::AppMention { channel, user, bot_id, text, files } => {
                    if bot_id.is_some() { return Ok(InboundEvent::Ignored); }
                    let media = extract_media(files);
                    let text = text.unwrap_or_default();
                    if text.is_empty() && media.is_empty() {
                        return Ok(InboundEvent::Ignored);
                    }
                    Ok(InboundEvent::Message { chat_id: bound_chat(&channel, user.as_deref())?, text, media })
                }
                Event::Other => Ok(InboundEvent::Ignored),
            },
            Envelope::Other => Ok(InboundEvent::Ignored),
        }
    }

    async fn download_media(
        &self, config: &JsonValue, media_ref: &MediaRef,
    ) -> Option<(Bytes, String, String)> {
        let token = Self::token(config).ok()?;
        // Slack doesn't expose url_private in the event payload reliably; fetch via files.info
        let info: JsonValue = HTTP
            .get(format!("{SLACK_API}/files.info?file={}", media_ref.provider_file_id))
            .bearer_auth(token).send().await.ok()?
            .json().await.ok()?;
        let url = info.pointer("/file/url_private")?.as_str()?;
        let bytes = HTTP.get(url).bearer_auth(token)
            .send().await.ok()?.bytes().await.ok()?;
        let content_type = media_ref.content_type.clone()
            .unwrap_or_else(|| "application/octet-stream".into());
        let name = media_ref.name.clone().unwrap_or_else(|| "file".into());
        Some((bytes, content_type, name))
    }

    async fn send_response(
        &self, config: &JsonValue, chat_id: &str, text: &str,
    ) -> Result<(), ChannelError> {
        self.api_post(config, "chat.postMessage", &json!({
            "channel": delivery_chat(chat_id)?, "text": text,
        })).await?;
        Ok(())
    }

    async fn send_approval(
        &self, config: &JsonValue, chat_id: &str, approval_id: &str,
        tool_name: &str, args: &JsonValue,
    ) -> Result<(), ChannelError> {
        self.api_post(config, "chat.postMessage", &json!({
            "channel": delivery_chat(chat_id)?,
            "text": format!("Tool approval: {tool_name}"),
            "blocks": [
                { "type": "section", "text": { "type": "mrkdwn",
                    "text": format!("⚙️ *{tool_name}*\n```\n{args}\n```") } },
                { "type": "actions", "elements": [
                    { "type": "button", "text": { "type": "plain_text", "text": "✅ Approve" },
                      "value": format!("approve:{approval_id}"), "action_id": "approve",
                      "style": "primary" },
                    { "type": "button", "text": { "type": "plain_text", "text": "❌ Deny" },
                      "value": format!("deny:{approval_id}"), "action_id": "deny",
                      "style": "danger" },
                ]},
            ],
        })).await?;
        Ok(())
    }

    async fn send_choice(
        &self, config: &JsonValue, chat_id: &str, text: &str,
        options: &[(String, String)],
    ) -> Result<(), ChannelError> {
        let elements: Vec<JsonValue> = options.iter().enumerate().map(|(i, (label, value))| json!({
            "type": "button",
            "text": { "type": "plain_text", "text": label },
            "value": value,
            "action_id": format!("opt_{i}"),
        })).collect();
        self.api_post(config, "chat.postMessage", &json!({
            "channel": delivery_chat(chat_id)?, "text": text,
            "blocks": [
                { "type": "section", "text": { "type": "mrkdwn", "text": text } },
                { "type": "actions", "elements": elements },
            ],
        })).await?;
        Ok(())
    }

    async fn answer_callback(&self, _config: &JsonValue, callback_id: &str, text: &str) -> Result<(), ChannelError> {
        // callback_id holds Slack's response_url for interactive payloads
        if callback_id.is_empty() { return Ok(()); }
        HTTP.post(callback_id)
            .json(&json!({ "text": text, "response_type": "ephemeral", "replace_original": false }))
            .send().await
            .map_err(|e| ChannelError::Provider(e.to_string()))?;
        Ok(())
    }

    async fn register_webhook(
        &self, config: &JsonValue, _callback_url: &str,
    ) -> Result<(), ChannelError> {
        // Slack Request URL is configured manually in the app dashboard.
        // We just validate that the bot token works.
        self.api_post(config, "auth.test", &json!({})).await?;
        Ok(())
    }

    async fn unregister_webhook(&self, _config: &JsonValue) -> Result<(), ChannelError> { Ok(()) }

    async fn resolve_bot_meta(&self, config: &JsonValue) -> Option<JsonValue> {
        let resp = self.api_post(config, "auth.test", &json!({})).await.ok()?;
        Some(json!({
            "bot_user_id": resp.get("user_id")?.as_str()?,
            "bot_username": resp.get("user").and_then(|v| v.as_str()).unwrap_or(""),
            "team_id": resp.get("team_id")?.as_str()?,
        }))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_url_validation() {
        let cases: &[(&str, bool)] = &[
            ("https://hooks.slack.com/actions/T/123/xyz", true),
            ("https://hooks.slack.com/", true),
            ("", true), // empty = no callback, allowed
            ("https://evil.com/hooks.slack.com", false),
            ("http://hooks.slack.com/actions/T/123/xyz", false), // non-HTTPS
            ("https://hooks.slack.com.evil.com/", false),
            ("https://169.254.169.254/latest/meta-data/", false),
        ];
        for (url, expect_ok) in cases {
            let result = validate_response_url(if url.is_empty() { None } else { Some(url.to_string()) });
            assert_eq!(result.is_ok(), *expect_ok, "url '{url}': expected ok={expect_ok}, got {}", result.is_ok());
        }
    }

    fn signed_headers(secret: &str, body: &[u8], ts: i64) -> HeaderMap {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"v0:");
        mac.update(ts.to_string().as_bytes());
        mac.update(b":");
        mac.update(body);
        let sig = format!("v0={}", hex::encode(mac.finalize().into_bytes()));
        let mut h = HeaderMap::new();
        h.insert("x-slack-request-timestamp", ts.to_string().parse().unwrap());
        h.insert("x-slack-signature", sig.parse().unwrap());
        h.insert("content-type", "application/json".parse().unwrap());
        h
    }

    fn now() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
    }

    async fn parse_with(config: JsonValue, headers: HeaderMap, body: Bytes) -> Result<InboundEvent, ChannelError> {
        SlackProvider::new().parse_webhook(&config, body, &headers).await
    }

    async fn signed_event(body: JsonValue) -> Result<InboundEvent, ChannelError> {
        let body = serde_json::to_vec(&body).unwrap();
        parse_with(json!({"signing_secret": "s"}), signed_headers("s", &body, now()), body.into()).await
    }

    async fn signed_form(fields: &[(&str, String)]) -> Result<InboundEvent, ChannelError> {
        let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields.iter().map(|(key, value)| (*key, value.as_str()))).finish();
        let mut headers = signed_headers("s", body.as_bytes(), now());
        headers.insert("content-type", "application/x-www-form-urlencoded".parse().unwrap());
        parse_with(json!({"signing_secret": "s"}), headers, body.into()).await
    }

    #[tokio::test]
    async fn room_members_have_distinct_identities_consistent_across_messages_links_and_callbacks() {
        let mut identities = Vec::new();
        for user in ["U1", "U2"] {
            let mention = signed_event(json!({"type":"event_callback", "event":{
                "type":"app_mention", "channel":"C42", "user":user, "text":"hello"
            }})).await.unwrap();
            let InboundEvent::Message { chat_id, .. } = mention else { panic!("expected message"); };
            let link = signed_form(&[("command", "/link".into()), ("text", "link_token".into()),
                ("channel_id", "C42".into()), ("user_id", user.into())]).await.unwrap();
            let InboundEvent::Message { chat_id: linked, .. } = link else { panic!("expected link"); };
            let callback = signed_form(&[("payload", json!({
                "actions":[{"value":"approve:pending"}], "user":{"id":user}, "channel":{"id":"C42"}
            }).to_string())]).await.unwrap();
            let InboundEvent::Callback { chat_id: replied, .. } = callback else { panic!("expected callback"); };
            assert_eq!(chat_id, linked, "link identity for {user}");
            assert_eq!(chat_id, replied, "callback identity for {user}");
            assert_eq!(delivery_chat(&chat_id).unwrap(), "C42", "delivery for {user}");
            identities.push(chat_id);
        }
        assert_ne!(identities[0], identities[1], "a shared room cannot share a Core identity");
    }

    #[tokio::test]
    async fn missing_sender_or_room_cannot_create_an_executable_event() {
        for event_type in ["message", "app_mention"] {
            let result = signed_event(json!({"type":"event_callback", "event":{
                "type":event_type, "channel":"D42", "channel_type":"im", "text":"/link link_token"
            }})).await;
            assert!(result.is_err(), "{event_type}");
        }
        assert!(signed_form(&[("command", "/link".into()), ("channel_id", "C42".into()), ("text", "link_token".into())]).await.is_err());
        for payload in [
            json!({"actions":[{"value":"approve:a"}], "channel":{"id":"C42"}}),
            json!({"actions":[{"value":"approve:a"}], "user":{"id":"U1"}}),
        ] {
            assert!(signed_form(&[("payload", payload.to_string())]).await.is_err(), "{payload}");
        }
    }

    #[tokio::test]
    async fn bot_mentions_are_ignored() {
        let event = signed_event(json!({"type":"event_callback", "event":{
            "type":"app_mention", "channel":"C42", "user":"UBOT", "bot_id":"B1", "text":"hello"
        }})).await.unwrap();
        assert!(matches!(event, InboundEvent::Ignored));
    }

    #[test]
    fn delivery_never_falls_back_to_legacy_or_incomplete_identities() {
        for identity in ["C42", "U1", "slack:C42", "slack:[\"C42\",\"\"]", "slack:[\"\",\"U1\"]", "telegram:[42,1]"] {
            assert!(delivery_chat(identity).is_err(), "{identity}");
        }
    }

    #[tokio::test]
    async fn url_verification_returns_reply_with_challenge() {
        let body = serde_json::to_vec(&json!({
            "type": "url_verification", "challenge": "abc123"
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now());
        let event = parse_with(cfg, headers, body.into()).await.unwrap();
        let InboundEvent::Reply(v) = event else { panic!("expected Reply") };
        assert_eq!(v["challenge"], "abc123");
    }

    #[tokio::test]
    async fn dm_text_message_returns_message_event() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "event": { "type": "message", "user": "U1", "channel": "D123",
                       "channel_type": "im", "text": "hello" }
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now());
        let event = parse_with(cfg, headers, body.into()).await.unwrap();
        let InboundEvent::Message { chat_id, text, media } = event else { panic!("expected Message") };
        assert_eq!(chat_id, r#"slack:["D123","U1"]"#);
        assert_eq!(text, "hello");
        assert!(media.is_empty());
    }

    #[tokio::test]
    async fn channel_message_outside_im_is_ignored() {
        // v1: only DMs are handled; messages in public channels (C…) must be ignored.
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "event": { "type": "message", "user": "U1", "channel": "C123",
                       "channel_type": "channel", "text": "hi" }
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now());
        assert!(matches!(parse_with(cfg, headers, body.into()).await.unwrap(), InboundEvent::Ignored));
    }

    #[tokio::test]
    async fn bots_own_messages_are_ignored() {
        // Avoid feedback loop when the bot posts.
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "event": { "type": "message", "user": "U1", "channel": "D123",
                       "channel_type": "im", "text": "echo", "bot_id": "B999" }
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now());
        assert!(matches!(parse_with(cfg, headers, body.into()).await.unwrap(), InboundEvent::Ignored));
    }

    #[tokio::test]
    async fn message_subtype_is_ignored() {
        // Edits, deletions, joins all carry a subtype; we don't process them.
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "event": { "type": "message", "user": "U1", "channel": "D123",
                       "channel_type": "im", "subtype": "message_changed" }
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now());
        assert!(matches!(parse_with(cfg, headers, body.into()).await.unwrap(), InboundEvent::Ignored));
    }

    #[tokio::test]
    async fn app_mention_extracts_message() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "event": { "type": "app_mention", "user": "U1", "channel": "C42", "text": "<@U1> hey" }
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now());
        let event = parse_with(cfg, headers, body.into()).await.unwrap();
        let InboundEvent::Message { chat_id, text, .. } = event else { panic!("expected Message") };
        assert_eq!(chat_id, r#"slack:["C42","U1"]"#);
        assert_eq!(text, "<@U1> hey");
    }

    #[tokio::test]
    async fn message_with_files_extracts_media() {
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "event": { "type": "message", "user": "U1", "channel": "D1", "channel_type": "im",
                       "text": "look",
                       "files": [{ "id": "F1", "name": "report.pdf", "mimetype": "application/pdf" }] }
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now());
        let event = parse_with(cfg, headers, body.into()).await.unwrap();
        let InboundEvent::Message { media, .. } = event else { panic!("expected Message") };
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].provider_file_id, "F1");
        assert_eq!(media[0].name.as_deref(), Some("report.pdf"));
    }

    #[tokio::test]
    async fn signature_mismatch_rejected() {
        let body = b"{}".to_vec();
        let mut headers = signed_headers("wrong_secret", &body, now());
        let cfg = json!({ "signing_secret": "right_secret" });
        // tweak the signature to be valid format but wrong key
        let result = parse_with(cfg, headers.clone(), body.clone().into()).await;
        assert!(matches!(result, Err(ChannelError::InvalidWebhook(_))));

        // also reject if signature header missing entirely
        headers.remove("x-slack-signature");
        let result = parse_with(json!({ "signing_secret": "s" }), headers, body.into()).await;
        assert!(matches!(result, Err(ChannelError::InvalidWebhook(_))));
    }

    #[tokio::test]
    async fn stale_timestamp_rejected() {
        // Replay protection: timestamps older than 5 minutes must be rejected.
        // Use an event_callback body — url_verification skips sig check by design.
        let body = serde_json::to_vec(&json!({
            "type": "event_callback",
            "event": { "type": "message", "user": "U1", "channel": "D1", "channel_type": "im", "text": "hi" }
        })).unwrap();
        let cfg = json!({ "signing_secret": "s" });
        let headers = signed_headers("s", &body, now() - 600);
        let result = parse_with(cfg, headers, body.into()).await;
        assert!(matches!(result, Err(ChannelError::InvalidWebhook(_))));
    }

    #[tokio::test]
    async fn url_verification_works_without_signing_secret() {
        // Critical: at app creation time the user hasn't pasted signing_secret
        // yet, so the channel config is empty. URL verification must still work.
        let body = serde_json::to_vec(&json!({
            "type": "url_verification", "challenge": "abc"
        })).unwrap();
        let event = parse_with(json!({}), HeaderMap::new(), body.into()).await.unwrap();
        let InboundEvent::Reply(v) = event else { panic!("expected Reply") };
        assert_eq!(v["challenge"], "abc");
    }

    #[tokio::test]
    async fn interactive_payload_returns_callback() {
        let payload = json!({
            "actions": [{ "value": "approve:abc123" }],
            "user": { "id": "U1" },
            "channel": { "id": "D1" },
            "response_url": "https://hooks.slack.com/actions/T/123/xyz"
        });
        let body = format!("payload={}", url::form_urlencoded::byte_serialize(
            payload.to_string().as_bytes()).collect::<String>());
        let cfg = json!({ "signing_secret": "s" });
        let mut headers = signed_headers("s", body.as_bytes(), now());
        headers.insert("content-type", "application/x-www-form-urlencoded".parse().unwrap());
        // re-sign because content-type doesn't affect HMAC, ts/sig already correct
        let event = parse_with(cfg, headers, body.into()).await.unwrap();
        let InboundEvent::Callback { chat_id, callback_id, data } = event else { panic!("expected Callback") };
        assert_eq!(chat_id, r#"slack:["D1","U1"]"#);
        assert_eq!(data, "approve:abc123");
        assert!(callback_id.starts_with("https://hooks.slack.com"));
    }

    #[tokio::test]
    async fn interactive_payload_with_bad_response_url_rejected() {
        // Attacker-controlled response_url pointing at internal/SSRF target must be rejected.
        for bad_url in &[
            "https://evil.com/steal",
            "http://hooks.slack.com/actions/x",
            "https://169.254.169.254/latest/meta-data/",
        ] {
            let payload = json!({
                "actions": [{ "value": "approve:abc123" }],
                "user": { "id": "U1" },
                "channel": { "id": "D1" },
                "response_url": bad_url
            });
            let body = format!("payload={}", url::form_urlencoded::byte_serialize(
                payload.to_string().as_bytes()).collect::<String>());
            let cfg = json!({ "signing_secret": "s" });
            let mut headers = signed_headers("s", body.as_bytes(), now());
            headers.insert("content-type", "application/x-www-form-urlencoded".parse().unwrap());
            let result = parse_with(cfg, headers, body.into()).await;
            assert!(matches!(result, Err(ChannelError::InvalidWebhook(_))), "expected rejection for {bad_url}");
        }
    }
}
