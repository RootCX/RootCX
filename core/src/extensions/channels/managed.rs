use super::types::{ChannelError, ChannelProvider, InboundEvent};
use crate::{api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};
use async_trait::async_trait;
use axum::{
    Json,
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;

pub struct ManagedProvider {
    pub name: String,
}

pub(crate) const AGENT_APP: &str = "assistant";

fn env(key: &str) -> Result<String, ChannelError> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ChannelError::Provider(format!("{key} is not configured")))
}

fn relay_key() -> Result<String, ChannelError> {
    env("ROOTCX_CHANNEL_RELAY_KEY").or_else(|_| env("ROOTCX_WHATSAPP_RELAY_KEY"))
}

fn tenant() -> Result<String, ChannelError> {
    env("ROOTCX_WHATSAPP_TENANT_REF").or_else(|_| env("ROOTCX_TENANT_REF"))
}

fn signature(key: &str, context: &str, timestamp: &str, body: &[u8]) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key size");
    mac.update(format!("{context}\n{timestamp}\n").as_bytes());
    mac.update(body);
    mac
}

fn verify(
    key: &str,
    context: &str,
    body: &[u8],
    headers: &HeaderMap,
    now: i64,
) -> Result<(), ChannelError> {
    let invalid = || ChannelError::InvalidWebhook("invalid relay signature".into());
    let timestamp = headers
        .get("x-rootcx-timestamp")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;
    let time: i64 = timestamp.parse().map_err(|_| invalid())?;
    if now.abs_diff(time) > 300 {
        return Err(invalid());
    }
    let digest = headers
        .get("x-rootcx-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(invalid)?;
    let bytes = hex::decode(digest).map_err(|_| invalid())?;
    signature(key, context, timestamp, body)
        .verify_slice(&bytes)
        .map_err(|_| invalid())
}

async fn relay(provider: &str, action: &str, mut payload: Value) -> Result<Value, ChannelError> {
    let tenant = tenant()?;
    let key = relay_key()?;
    let base = env("ROOTCX_CHANNEL_GATEWAY_URL").or_else(|_| env("ROOTCX_WHATSAPP_GATEWAY_URL"))?;
    payload["provider"] = json!(provider);
    let body = payload.to_string();
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let digest = hex::encode(
        signature(
            &key,
            &format!("gateway:{action}:{tenant}"),
            &timestamp,
            body.as_bytes(),
        )
        .finalize()
        .into_bytes(),
    );
    let response = reqwest::Client::new()
        .post(format!("{}/{action}", base.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(20))
        .header("content-type", "application/json")
        .header("x-rootcx-tenant", tenant)
        .header("x-rootcx-timestamp", timestamp)
        .header("x-rootcx-signature", digest)
        .body(body)
        .send()
        .await
        .map_err(|_| ChannelError::Provider("channel gateway unavailable".into()))?;
    if !response.status().is_success() {
        return Err(ChannelError::Provider(format!(
            "channel gateway returned {}",
            response.status()
        )));
    }
    response
        .json()
        .await
        .map_err(|_| ChannelError::Provider("invalid gateway response".into()))
}

pub async fn bootstrap(pool: &PgPool) -> Result<(), crate::RuntimeError> {
    sqlx::query("CREATE TABLE IF NOT EXISTS rootcx_system.whatsapp_receipts (
        message_id TEXT PRIMARY KEY, channel_id UUID NOT NULL REFERENCES rootcx_system.channels(id) ON DELETE CASCADE,
        received_at TIMESTAMPTZ NOT NULL DEFAULT now(), status TEXT NOT NULL DEFAULT 'processing'
    )").execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    let gateway = std::env::var("ROOTCX_CHANNEL_GATEWAY_URL")
        .or_else(|_| std::env::var("ROOTCX_WHATSAPP_GATEWAY_URL"))
        .unwrap_or_default();
    if gateway.is_empty() {
        return Ok(());
    }
    relay_key().map_err(|e| crate::RuntimeError::Schema(sqlx::Error::Protocol(e.to_string())))?;
    let tenant =
        tenant().map_err(|e| crate::RuntimeError::Schema(sqlx::Error::Protocol(e.to_string())))?;
    for (provider, label, public_key, value) in [
        (
            "whatsapp",
            "WhatsApp",
            "phone_number",
            std::env::var("ROOTCX_WHATSAPP_PHONE_NUMBER").unwrap_or_default(),
        ),
        (
            "telegram",
            "Telegram",
            "bot_username",
            std::env::var("ROOTCX_CHANNEL_TELEGRAM_USERNAME").unwrap_or_default(),
        ),
    ] {
        if value.is_empty() {
            continue;
        }
        let id = uuid::Uuid::new_v5(
            &super::CHANNEL_UUID_NAMESPACE,
            format!("{provider}:{tenant}").as_bytes(),
        );
        let mut config =
            json!({"managed":true,"linking_version":2,"channel_id":id,"default_agent":AGENT_APP});
        config[public_key] = json!(value);
        sqlx::query(
            "INSERT INTO rootcx_system.channels(id,provider,name,config,status)
            VALUES($1,$2,$3,$4,'active') ON CONFLICT(id) DO UPDATE SET config=EXCLUDED.config",
        )
        .bind(id)
        .bind(provider)
        .bind(format!("Shappy — {label}"))
        .bind(config)
        .execute(pool)
        .await
        .map_err(crate::RuntimeError::Schema)?;
    }
    Ok(())
}

pub async fn authorize(
    provider: &str,
    channel: &str,
    grant: &str,
    user: uuid::Uuid,
    binding: bool,
) -> Result<(), ChannelError> {
    let result = relay(provider, "authorize", json!({"channelId":channel,"to": grant, "coreUserId": user, "operation": if binding { "bind" } else { "invoke" }})).await?;
    if result["authorized"] != true
        || result["grantId"] != grant
        || result["coreUserId"] != user.to_string()
    {
        return Err(ChannelError::Provider(
            "channel authorization refused".into(),
        ));
    }
    Ok(())
}

pub async fn link(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(channel): Path<String>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let grant = body["grantId"]
        .as_str()
        .filter(|s| uuid::Uuid::parse_str(s).is_ok())
        .ok_or_else(|| ApiError::BadRequest("grant required".into()))?;
    let row: Option<(String, Value)> = sqlx::query_as("SELECT provider,config FROM rootcx_system.channels WHERE id=$1::uuid AND status='active' AND config->>'managed'='true'")
        .bind(&channel).fetch_optional(rt.pool()).await?;
    let (provider, config) =
        row.ok_or_else(|| ApiError::BadRequest("managed channel required".into()))?;
    authorize(&provider, &channel, grant, identity.user_id, true)
        .await
        .map_err(|_| ApiError::Forbidden("channel association not authorized".into()))?;
    let agent = crate::extensions::agents::agent_user_id(
        config["default_agent"].as_str().unwrap_or(AGENT_APP),
    );
    let trigger = super::channel_delegation_ref(&channel, identity.user_id);
    let mut tx = rt.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("channel-link:{trigger}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM rootcx_system.channel_sessions WHERE channel_id = $1::uuid AND external_chat_id IN (SELECT external_chat_id FROM rootcx_system.channel_identities WHERE channel_id = $1::uuid AND user_id = $2 AND external_chat_id != $3)")
        .bind(&channel).bind(identity.user_id).bind(grant).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM rootcx_system.channel_identities WHERE channel_id = $1::uuid AND user_id = $2 AND external_chat_id != $3")
        .bind(&channel).bind(identity.user_id).bind(grant).execute(&mut *tx).await?;
    // A collision cannot transfer an existing chat to another principal.
    sqlx::query("INSERT INTO rootcx_system.channel_identities(channel_id, external_chat_id, user_id) VALUES ($1::uuid, $2, $3) ON CONFLICT (channel_id, external_chat_id) DO NOTHING")
        .bind(&channel).bind(grant).bind(identity.user_id).execute(&mut *tx).await?;
    let owner: uuid::Uuid = sqlx::query_scalar("SELECT user_id FROM rootcx_system.channel_identities WHERE channel_id = $1::uuid AND external_chat_id = $2")
        .bind(&channel).bind(grant).fetch_one(&mut *tx).await?;
    if owner != identity.user_id {
        return Err(ApiError::Forbidden("grant owner mismatch".into()));
    }
    sqlx::query("UPDATE rootcx_system.delegations SET revoked_at = now() WHERE trigger_type = 'channel' AND trigger_ref = $1 AND revoked_at IS NULL")
        .bind(trigger).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO rootcx_system.delegations(delegator_uid, delegatee_uid, trigger_type, trigger_ref) VALUES ($1, $2, 'channel', $3)")
        .bind(identity.user_id).bind(agent).bind(trigger).execute(&mut *tx).await?;
    authorize(&provider, &channel, grant, identity.user_id, true)
        .await
        .map_err(|_| ApiError::Forbidden("channel association revoked".into()))?;
    tx.commit().await?;
    Ok(Json(json!({"linked": true})))
}

#[async_trait]
impl ChannelProvider for ManagedProvider {
    fn delivery_id(&self, body: &[u8]) -> Result<String, ChannelError> {
        let payload: Value = serde_json::from_slice(body)
            .map_err(|_| ChannelError::InvalidWebhook("invalid payload".into()))?;
        payload["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ChannelError::InvalidWebhook("missing delivery id".into()))
    }

    async fn authorize_session(
        &self,
        config: &Value,
        chat: &str,
        user: uuid::Uuid,
    ) -> Result<(), ChannelError> {
        authorize(
            &self.name,
            config["channel_id"].as_str().unwrap_or(""),
            chat,
            user,
            false,
        )
        .await
    }

    async fn parse_webhook(
        &self,
        config: &Value,
        body: Bytes,
        headers: &HeaderMap,
    ) -> Result<InboundEvent, ChannelError> {
        let channel = config["channel_id"]
            .as_str()
            .ok_or_else(|| ChannelError::InvalidWebhook("unmanaged channel".into()))?;
        verify(
            &relay_key()?,
            &format!("core:{channel}"),
            &body,
            headers,
            chrono::Utc::now().timestamp(),
        )?;
        let payload: Value = serde_json::from_slice(&body)
            .map_err(|_| ChannelError::InvalidWebhook("invalid payload".into()))?;
        let chat = payload["chat_id"]
            .as_str()
            .filter(|s| uuid::Uuid::parse_str(s).is_ok())
            .ok_or_else(|| ChannelError::InvalidWebhook("invalid sender".into()))?;
        if let Some(signed_data) = payload["callback"].as_str() {
            let (data, digest) = signed_data
                .rsplit_once(':')
                .ok_or_else(|| ChannelError::InvalidWebhook("invalid approval".into()))?;
            let digest = hex::decode(digest)
                .map_err(|_| ChannelError::InvalidWebhook("invalid approval".into()))?;
            signature(&relay_key()?, &format!("approval:{chat}"), data, b"")
                .verify_slice(&digest)
                .map_err(|_| {
                    ChannelError::InvalidWebhook("approval belongs to another recipient".into())
                })?;
            return Ok(InboundEvent::Callback {
                chat_id: chat.into(),
                callback_id: String::new(),
                data: data.into(),
            });
        }
        let text = payload["text"]
            .as_str()
            .ok_or_else(|| ChannelError::InvalidWebhook("text required".into()))?;
        Ok(InboundEvent::Message {
            chat_id: chat.into(),
            text: text.into(),
            media: vec![],
        })
    }

    async fn send_response(
        &self,
        config: &Value,
        chat: &str,
        text: &str,
    ) -> Result<(), ChannelError> {
        // Keep a prefix budget for the gateway, and never split a UTF-8 code point.
        for chunk in text.chars().collect::<Vec<_>>().chunks(3700) {
            relay(&self.name, "send", json!({"channelId": config["channel_id"], "to": chat, "kind": "text", "text": chunk.iter().collect::<String>()})).await?;
        }
        Ok(())
    }

    async fn send_approval(
        &self,
        config: &Value,
        chat: &str,
        approval: &str,
        tool: &str,
        args: &Value,
    ) -> Result<(), ChannelError> {
        let key = relay_key()?;
        let button_id = |action: &str| {
            let data = format!("{action}:{approval}");
            let digest = hex::encode(
                signature(&key, &format!("approval:{chat}"), &data, b"")
                    .finalize()
                    .into_bytes(),
            );
            format!("{data}:{digest}")
        };
        let details = serde_json::to_string_pretty(args).unwrap_or_default();
        // A truncated operation is not informed consent. Ask for SHAPP review instead.
        if details.chars().count() > 650 {
            self.send_response(config, chat, "Cette modification est trop détaillée pour être confirmée ici. Effectuez-la depuis SHAPP.").await?;
            return Err(ChannelError::Provider(
                "approval details exceed channel limit".into(),
            ));
        }
        relay(
            &self.name,
            "send",
            json!({"channelId": config["channel_id"], "to": chat, "kind":"confirmation",
                "text":format!("Confirmer {tool} ?\n{details}\n\nValable 10 minutes."),
                "choices":[
                    {"id":button_id("approve"),"title":"Autoriser"},
                    {"id":button_id("deny"),"title":"Refuser"}
                ]
            }),
        )
        .await?;
        Ok(())
    }

    async fn send_choice(
        &self,
        config: &Value,
        chat: &str,
        text: &str,
        options: &[(String, String)],
    ) -> Result<(), ChannelError> {
        let lines = options
            .iter()
            .map(|(label, _)| format!("/agent {label}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.send_response(config, chat, &format!("{text}\n{lines}"))
            .await
    }

    async fn register_webhook(&self, _: &Value, _: &str) -> Result<(), ChannelError> {
        env("ROOTCX_CHANNEL_GATEWAY_URL").or_else(|_| env("ROOTCX_WHATSAPP_GATEWAY_URL"))?;
        Ok(())
    }
    async fn unregister_webhook(&self, _: &Value) -> Result<(), ChannelError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relay_authentication_binds_channel_timestamp_and_body() {
        let body = b"{\"chat_id\":\"32470000000\"}";
        let digest = hex::encode(
            signature("tenant-secret", "core:channel-a", "1000", body)
                .finalize()
                .into_bytes(),
        );
        let mut headers = HeaderMap::new();
        headers.insert("x-rootcx-timestamp", "1000".parse().unwrap());
        headers.insert("x-rootcx-signature", digest.parse().unwrap());
        assert!(verify("tenant-secret", "core:channel-a", body, &headers, 1000).is_ok());
        for (key, context, bytes, now) in [
            ("other-tenant", "core:channel-a", body.as_slice(), 1000),
            ("tenant-secret", "core:channel-b", body.as_slice(), 1000),
            (
                "tenant-secret",
                "core:channel-a",
                b"tampered".as_slice(),
                1000,
            ),
            ("tenant-secret", "core:channel-a", body.as_slice(), 1301),
            ("tenant-secret", "core:channel-a", body.as_slice(), 699),
        ] {
            assert!(
                verify(key, context, bytes, &headers, now).is_err(),
                "{key} {context} {now}"
            );
        }
    }
}
