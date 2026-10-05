use crate::governance::execution::AgentSource;
use super::origin::ChannelSession;
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use futures::future::join_all;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use tokio::sync::Mutex;
use tracing::{error, info};

use super::types::{ChannelProvider, InboundEvent, MediaRef};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::agents::{self, config as agent_config, persistence};
use crate::extensions::agents::approvals::ApprovalResponse;
use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};
use crate::ipc::{AgentInvokePayload, FileAttachment, LlmModelRef};
use crate::routes::{self, SharedRuntime, llm_models::fetch_default_llm};

#[derive(Deserialize)]
pub struct CreateChannel {
    provider: String,
    name: String,
    config: JsonValue,
}

pub async fn create_channel(
    _identity: Identity, State(rt): State<SharedRuntime>, Json(body): Json<CreateChannel>,
) -> Result<Json<JsonValue>, ApiError> {
    if body.config["managed"] == true {
        return Err(ApiError::BadRequest("managed channels are provisioned by the platform".into()));
    }
    if super::provider(&body.provider).is_none() {
        return Err(ApiError::BadRequest(format!("unsupported provider: {}", body.provider)));
    }
    let pool = routes::pool(&rt);
    let id = uuid::Uuid::new_v4().to_string();
    let webhook_secret = uuid::Uuid::new_v4().to_string().replace('-', "");
    let mut config = body.config;
    config["webhook_secret"] = json!(webhook_secret);

    sqlx::query(
        "INSERT INTO rootcx_system.channels (id, provider, name, config, status)
         VALUES ($1::uuid, $2, $3, $4, 'inactive')",
    ).bind(&id).bind(&body.provider).bind(&body.name).bind(&config)
    .execute(&pool).await?;

    info!(channel_id = %id, provider = %body.provider, "channel created");
    Ok(Json(json!({ "id": id, "webhook_secret": webhook_secret })))
}

pub async fn list_channels(
    _identity: Identity, State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = routes::pool(&rt);
    let rows: Vec<(String, String, String, JsonValue, String, String, String)> = sqlx::query_as(
        "SELECT id::text, provider, name, config, status, created_at::text, updated_at::text
         FROM rootcx_system.channels ORDER BY created_at DESC",
    ).fetch_all(&pool).await?;

    Ok(Json(rows.into_iter().map(|(id, provider, name, config, status, ca, ua)| {
        json!({ "id": id, "provider": provider, "name": name, "config": redact_config(config), "status": status, "createdAt": ca, "updatedAt": ua })
    }).collect()))
}

pub async fn delete_channel(
    _identity: Identity, State(rt): State<SharedRuntime>, Path(channel_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    if let Some((prov, cfg)) = sqlx::query_as::<_, (String, JsonValue)>(
        "SELECT provider, config FROM rootcx_system.channels WHERE id = $1::uuid",
    ).bind(&channel_id).fetch_optional(&pool).await? {
        if let Some(p) = super::provider_for(&prov, &cfg) { let _ = p.unregister_webhook(&cfg).await; }
    }
    sqlx::query("DELETE FROM rootcx_system.channels WHERE id = $1::uuid")
        .bind(&channel_id).execute(&pool).await?;
    info!(channel_id, "channel deleted");
    Ok(Json(json!({ "status": "ok" })))
}

fn load_channel(r: Option<(String, JsonValue)>, id: &str) -> Result<(String, JsonValue), ApiError> {
    r.ok_or_else(|| ApiError::NotFound(format!("channel '{id}' not found")))
}

#[derive(Deserialize, Default)]
pub struct ActivateChannel {
    pub public_url: Option<String>,
    /// Optional config patch merged into the channel's existing config before
    /// activation. Used by the 2-step Slack flow: create channel empty → user
    /// fills in tokens later → activate with the merged config.
    pub config: Option<JsonValue>,
}

pub async fn activate_channel(
    _identity: Identity, State(rt): State<SharedRuntime>, Path(channel_id): Path<String>,
    body: Option<Json<ActivateChannel>>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let (prov, mut cfg) = load_channel(sqlx::query_as(
        "SELECT provider, config FROM rootcx_system.channels WHERE id = $1::uuid",
    ).bind(&channel_id).fetch_optional(&pool).await?, &channel_id)?;

    let body = body.map(|b| b.0).unwrap_or_default();

    if let Some(patch) = body.config {
        if patch.get("managed").is_some() || patch.get("channel_id").is_some() || patch.get("linking_version").is_some() {
            return Err(ApiError::BadRequest("platform channel configuration cannot be patched".into()));
        }
        let (mut creds, mut rest) = (serde_json::Map::new(), serde_json::Map::new());
        if let Some(obj) = patch.as_object() {
            for (k, v) in obj {
                if PROTECTED_CONFIG_KEYS.contains(&k.as_str()) { creds.insert(k.clone(), v.clone()); }
                else { rest.insert(k.clone(), v.clone()); }
            }
        }
        if !creds.is_empty() && write_credentials(&pool, &channel_id, JsonValue::Object(creds.clone())).await? {
            // Credentials were written — merge into cfg so the rest of the handler sees them.
            if let Some(base) = cfg.as_object_mut() { base.extend(creds); }
        }
        if !rest.is_empty() {
            cfg = apply_config_patch(cfg, JsonValue::Object(rest));
        }
    }

    let provider = super::provider_for(&prov, &cfg)
        .ok_or_else(|| ApiError::Internal(format!("unknown provider: {prov}")))?;
    let base = resolve_public_url(body.public_url)?;
    let url = format!("{base}/api/v1/channels/{prov}/{channel_id}/webhook");
    provider.register_webhook(&cfg, &url).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(obj) = provider.resolve_bot_meta(&cfg).await.and_then(|m| m.as_object().cloned()) {
        if let Some(base) = cfg.as_object_mut() { base.extend(obj); }
    }

    sqlx::query("UPDATE rootcx_system.channels SET config = $1, status = 'active', updated_at = now() WHERE id = $2::uuid")
        .bind(&cfg).bind(&channel_id).execute(&pool).await?;
    info!(channel_id, "channel activated, webhook: {url}");
    Ok(Json(json!({ "status": "active", "webhook_url": url })))
}

pub async fn deactivate_channel(
    _identity: Identity, State(rt): State<SharedRuntime>, Path(channel_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let (prov, cfg) = load_channel(sqlx::query_as(
        "SELECT provider, config FROM rootcx_system.channels WHERE id = $1::uuid",
    ).bind(&channel_id).fetch_optional(&pool).await?, &channel_id)?;

    if let Some(p) = super::provider_for(&prov, &cfg) { let _ = p.unregister_webhook(&cfg).await; }
    sqlx::query("UPDATE rootcx_system.channels SET status = 'inactive', updated_at = now() WHERE id = $1::uuid")
        .bind(&channel_id).execute(&pool).await?;
    info!(channel_id, "channel deactivated");
    Ok(Json(json!({ "status": "inactive" })))
}

pub async fn webhook(
    State(rt): State<SharedRuntime>,
    Path((provider_name, channel_id)): Path<(String, String)>,
    headers: HeaderMap, body: Bytes,
) -> Result<Json<JsonValue>, ApiError> {
    let (config, status): (JsonValue, String) = sqlx::query_as(
        "SELECT config, status FROM rootcx_system.channels WHERE id = $1::uuid AND provider = $2",
    ).bind(&channel_id).bind(&provider_name).fetch_optional(rt.pool()).await?
        .ok_or_else(|| ApiError::NotFound("channel not found".into()))?;
    let provider = super::provider_for(&provider_name, &config)
        .ok_or_else(|| ApiError::BadRequest("unknown channel provider".into()))?;
    let event = provider.parse_webhook(&config, body.clone(), &headers).await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    let (chat, is_message) = match &event {
        InboundEvent::Reply(value) => return Ok(Json(value.clone())),
        InboundEvent::Ignored => return Ok(Json(json!({"ok": true}))),
        InboundEvent::Message { chat_id, .. } => (chat_id.clone(), true),
        InboundEvent::Callback { chat_id, .. } => (chat_id.clone(), false),
    };
    if status != "active" { return Ok(Json(json!({"ok": true}))); }
    if config["managed"] == true {
        let user = resolve_invoker(rt.pool(), &channel_id, &chat).await
            .ok_or_else(|| ApiError::Forbidden("channel identity missing".into()))?;
        provider.authorize_session(&config, &chat, user).await
            .map_err(|_| ApiError::Forbidden("channel access revoked".into()))?;
    }
    let delivery = provider.delivery_id(&body).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if delivery.is_empty() || delivery.len() > 512 { return Err(ApiError::BadRequest("invalid delivery id".into())); }
    let channel = channel_id.parse().map_err(|_| ApiError::BadRequest("invalid channel".into()))?;
    match super::intake::admit(rt.pool(), channel, &delivery, &chat, is_message).await? {
        super::intake::Admission::Duplicate(status) => return Ok(Json(json!({"ok":true,"duplicate":true,"status":status}))),
        super::intake::Admission::Busy => return Err(ApiError::Conflict("conversation busy; resend after the current request".into())),
        super::intake::Admission::Ready => {}
    }
    // Managed gateways wait for a terminal receipt; native webhooks acknowledge promptly.
    if config["managed"] == true {
        let result = process_event(&rt, &channel_id, &provider_name, &config, provider.as_ref(), event).await;
        super::intake::finish(rt.pool(), channel, &delivery, result.is_ok()).await?;
        return Ok(Json(json!({"ok":true,"status":if result.is_ok() {"done"} else {"failed"}})));
    }
    tokio::spawn(async move {
        let result = process_event(&rt, &channel_id, &provider_name, &config, provider.as_ref(), event).await;
        if let Err(error) = &result { error!(channel_id, %error, "channel processing failed; do not replay actions"); }
        if let Err(error) = super::intake::finish(rt.pool(), channel, &delivery, result.is_ok()).await {
            error!(channel_id, %error, "channel receipt remains processing; operator reconciliation required");
        }
    });
    Ok(Json(json!({"ok":true,"status":"processing"})))
}

async fn process_event(
    rt: &SharedRuntime, channel: &str, name: &str, config: &JsonValue,
    provider: &dyn ChannelProvider, event: InboundEvent,
) -> Result<(), String> {
    match event {
        InboundEvent::Callback { chat_id, callback_id, data } => {
            handle_callback(rt, channel, config, provider, &chat_id, &callback_id, &data).await;
            Ok(())
        }
        InboundEvent::Message { chat_id, text, media } => {
            if config["managed"] != true && handle_command(rt.pool(), channel, &chat_id, &text, config, provider).await {
                return Ok(());
            }
            if resolve_invoker(rt.pool(), channel, &chat_id).await.is_none() {
                return provider.send_response(config, &chat_id, "Link your account from your dashboard before chatting with Shappy.")
                    .await.map_err(|e| e.to_string());
            }
            do_invoke(rt, channel, config, name, &chat_id, &text, media).await
        }
        _ => Ok(()),
    }
}

async fn do_invoke(
    rt: &SharedRuntime,
    channel_id: &str, config: &JsonValue, provider_name: &str,
    chat_id: &str, text: &str, media: Vec<MediaRef>,
) -> Result<(), String> {
    fn e(e: impl std::fmt::Debug) -> String { format!("{e:?}") }
    let pool = rt.pool();
    let wm = rt.worker_manager();
    let ((app_id, session_id), invoker_user_id) = tokio::try_join!(
        resolve_session(pool, channel_id, chat_id),
        async { Ok(resolve_invoker(pool, channel_id, chat_id).await) },
    ).map_err(&e)?;

    // Validate the particular channel link before loading history or media.
    // The same origin is rechecked at agent admission and before each tool.
    let human = invoker_user_id.ok_or("channel identity missing")?;
    let origin = ChannelSession::load(pool, channel_id, chat_id, human, &app_id).await?;

    let agent_cfg: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
    ).bind(&app_id).fetch_one(pool).await.map_err(&e)?;
    let memory = agent_cfg.pointer("/memory/enabled").and_then(JsonValue::as_bool) == Some(true);
    let history = agent_config::load_history(pool, memory, &app_id, &session_id).await.map_err(&e)?;
    let llm = fetch_default_llm(pool).await.map_err(&e)?
        .map(|(p, m)| LlmModelRef { provider: p, model: m });

    let provider = super::provider_for(provider_name, config).ok_or("unknown channel provider")?;
    let storage = PostgresBackend;
    let downloaded: Vec<_> = join_all(
        media.iter().map(|m| provider.download_media(config, m))
    ).await;
    let mut attachment_list: Vec<FileAttachment> = Vec::new();
    for result in downloaded.into_iter().flatten() {
        let (bytes, content_type, name) = result;
        let file_id = uuid::Uuid::new_v4();
        if let Err(err) = storage.put(pool, file_id, &app_id, &name, &content_type, &bytes, None).await {
            error!(channel_id, "failed to store media: {err}");
            continue;
        }
        let nonce = rt.upload_nonces().lock().unwrap_or_else(|e| e.into_inner())
            .create_download(file_id, &app_id);
        let url = crate::extensions::storage::download_url(rt.runtime_url(), &nonce);
        attachment_list.push(FileAttachment { name, content_type, url });
    }
    let attachments = if attachment_list.is_empty() { None } else { Some(attachment_list) };

    let payload = AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        message: text.to_string(),
        history, is_sub_invoke: false, llm, invoker_user_id,
        attachments,
        task_scope: None,
    };

    let mut rx = wm.agent_invoke(&app_id, payload, AgentSource::Origin(std::sync::Arc::new(origin))).await.map_err(&e)?;
    let typing = provider.start_typing(config, chat_id);

    let mut response = String::new();
    let mut tokens = None;
    while let Some(event) = rx.recv().await {
        match event {
            crate::worker::AgentEvent::Chunk { delta } => response.push_str(&delta),
            crate::worker::AgentEvent::Done { response: r, tokens: t } => { response = r; tokens = t; break; }
            crate::worker::AgentEvent::ApprovalRequired { approval_id, tool_name, args, .. } => {
                if provider.send_approval(config, chat_id, &approval_id, &tool_name, &args).await.is_err() {
                    rt.pending_approvals().cancel(&approval_id, "approval could not be delivered").await;
                }
            }
            crate::worker::AgentEvent::Error { error: e } => {
                if let Some(h) = &typing { h.abort(); }
                error!(channel_id, chat_id, "agent error: {e}");
                let _ = provider.send_response(config, chat_id, "La demande a échoué. Vérifiez le résultat dans SHAPP avant de réessayer.").await;
                return Err(format!("agent failed: {e}"));
            }
            _ => {}
        }
    }
    if let Some(h) = typing { h.abort(); }
    if response.is_empty() { return Ok(()); }

    if memory {
        let uid = agents::agent_user_id(&app_id);
        let _ = persistence::ensure_session(pool, &session_id, &app_id, uid).await;
        let _ = persistence::persist_message(pool, &session_id, "user", text, None, false).await;
        let _ = persistence::finalize_session(pool, &session_id, text, &response, tokens).await;
    }
    if let Err(e) = provider.send_response(config, chat_id, &response).await {
        error!(channel_id, chat_id, "send response failed: {e}");
        return Err(e.to_string());
    }
    Ok(())
}

const DEFAULT_AGENT: &str = "assistant";
const MSG_LINK_OK: &str = "Account linked! Your integrations are now available.";
const MSG_LINK_INVALID: &str = "Link expired or invalid. Please try again from the dashboard.";
const ACTION_APPROVE: &str = "approve";
const ACTION_DENY: &str = "deny";
const ACTION_AGENT: &str = "agent";
const PROTECTED_CONFIG_KEYS: &[&str] = &["bot_token", "webhook_secret", "signing_secret"];

fn parse_command(text: &str) -> Option<(&str, Vec<&str>)> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let cmd = parts.first().and_then(|p| p.split('@').next())?;
    Some((cmd, parts))
}

async fn all_agents(pool: &sqlx::PgPool) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT app_id, name FROM rootcx_system.agents ORDER BY name",
    ).fetch_all(pool).await.unwrap_or_default()
}

async fn create_session(
    pool: &sqlx::PgPool, channel_id: &str, chat_id: &str, app_id: &str,
) -> Result<String, ApiError> {
    let session_id = uuid::Uuid::new_v4().to_string();
    persistence::ensure_session(pool, &session_id, app_id, agents::agent_user_id(app_id)).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    sqlx::query(
        "INSERT INTO rootcx_system.channel_sessions (channel_id, external_chat_id, app_id, session_id)
         VALUES ($1::uuid, $2, $3, $4::uuid)
         ON CONFLICT (channel_id, external_chat_id)
         DO UPDATE SET app_id = EXCLUDED.app_id, session_id = EXCLUDED.session_id",
    ).bind(channel_id).bind(chat_id).bind(app_id).bind(&session_id)
    .execute(pool).await?;
    Ok(session_id)
}

async fn resolve_session(
    pool: &sqlx::PgPool, channel_id: &str, chat_id: &str,
) -> Result<(String, String), ApiError> {
    if let Some(row) = sqlx::query_as::<_, (String, String)>(
        "SELECT app_id, session_id::text FROM rootcx_system.channel_sessions
         WHERE channel_id = $1::uuid AND external_chat_id = $2",
    ).bind(channel_id).bind(chat_id).fetch_optional(pool).await? {
        return Ok(row);
    }
    let app_id: String = sqlx::query_scalar(
        "SELECT COALESCE(config->>'default_agent', $2) FROM rootcx_system.channels WHERE id = $1::uuid",
    ).bind(channel_id).bind(DEFAULT_AGENT).fetch_one(pool).await?;
    let session_id = create_session(pool, channel_id, chat_id, &app_id).await?;
    Ok((app_id, session_id))
}

async fn handle_callback(
    rt: &SharedRuntime, channel_id: &str, config: &JsonValue, provider: &dyn ChannelProvider,
    chat_id: &str, callback_id: &str, data: &str,
) {
    let (action, payload) = data.split_once(':').unwrap_or(("", ""));

    match action {
        ACTION_APPROVE => {
            handle_approval(rt, channel_id, config, provider, chat_id, callback_id, payload, ApprovalResponse::Approved, "Approved").await;
        }
        ACTION_DENY => {
            handle_approval(rt, channel_id, config, provider, chat_id, callback_id, payload,
                ApprovalResponse::Rejected { reason: "rejected via chat".into() }, "Denied").await;
        }
        ACTION_AGENT => {
            let pool = rt.pool();
            let found = all_agents(pool).await
                .into_iter().find(|(id, _)| id == payload);
            if let Some((app_id, name)) = found {
                if create_session(pool, channel_id, chat_id, &app_id).await.is_ok() {
                    let _ = provider.answer_callback(config, callback_id, &format!("Switched to {name}")).await;
                    let _ = provider.send_response(config, chat_id, &format!("Switched to *{name}*. New session started.")).await;
                } else {
                    let _ = provider.answer_callback(config, callback_id, "Failed to switch agent").await;
                }
            } else {
                let _ = provider.answer_callback(config, callback_id, "Agent not found").await;
            }
        }
        _ => { let _ = provider.answer_callback(config, callback_id, "Unknown action").await; }
    }
}

async fn reply_approval(
    rt: &SharedRuntime, channel: &str, chat: &str, approval: &str, response: ApprovalResponse,
) -> bool {
    let Some(human) = resolve_invoker(rt.pool(), channel, chat).await else { return false };
    let Ok((app, _)) = resolve_session(rt.pool(), channel, chat).await else { return false };
    let Ok(origin) = ChannelSession::load(rt.pool(), channel, chat, human, &app).await else { return false };
    rt.pending_approvals().reply(approval, &origin.owner(human), response).await
}

async fn handle_approval(
    rt: &SharedRuntime, channel_id: &str, config: &JsonValue, provider: &dyn ChannelProvider,
    chat_id: &str, callback_id: &str, approval_id: &str,
    response: ApprovalResponse, ack: &str,
) {
    let replied = reply_approval(rt, channel_id, chat_id, approval_id, response).await;
    let msg = if replied { ack } else { "Expired or already handled" };
    let _ = provider.answer_callback(config, callback_id, msg).await;
    if replied {
        let _ = provider.send_response(config, chat_id, &format!("_{ack}_")).await;
    }
}


async fn handle_command(
    pool: &sqlx::PgPool, channel_id: &str, chat_id: &str, text: &str,
    config: &JsonValue, provider: &dyn ChannelProvider,
) -> bool {
    let Some((cmd, parts)) = parse_command(text) else { return false };
    match cmd {
        "/newsession" => {
            let _ = sqlx::query(
                "DELETE FROM rootcx_system.channel_sessions
                 WHERE channel_id = $1::uuid AND external_chat_id = $2",
            ).bind(channel_id).bind(chat_id).execute(pool).await;
            let _ = provider.send_response(config, chat_id, "New session started.").await;
            true
        }
        "/start" | "/link" => {
            if let Some(token) = parts.get(1).filter(|t| t.starts_with(LINK_PREFIX)) {
                let msg = if try_complete_link(pool, channel_id, chat_id, token).await.is_some() {
                    MSG_LINK_OK
                } else {
                    MSG_LINK_INVALID
                };
                let _ = provider.send_response(config, chat_id, msg).await;
            } else {
                let _ = provider.send_response(config, chat_id, "Send me a message.").await;
            }
            true
        }
        "/agent" => {
            let agents = all_agents(pool).await;
            if parts.len() > 1 {
                let name = parts[1..].join(" ");
                if let Some((app_id, agent_name)) = agents.iter().find(|(_, n)| n.eq_ignore_ascii_case(&name)) {
                    let _ = create_session(pool, channel_id, chat_id, app_id).await;
                    let _ = provider.send_response(config, chat_id,
                        &format!("Switched to *{agent_name}*. New session started.")).await;
                } else {
                    let _ = provider.send_response(config, chat_id, "Agent not found.").await;
                }
            } else if agents.is_empty() {
                let _ = provider.send_response(config, chat_id, "No agents available.").await;
            } else {
                let options: Vec<(String, String)> = agents.into_iter()
                    .map(|(id, name)| (name, format!("{ACTION_AGENT}:{id}")))
                    .collect();
                let _ = provider.send_choice(config, chat_id, "Choose an agent:", &options).await;
            }
            true
        }
        _ => false,
    }
}

fn redact_config(mut config: JsonValue) -> JsonValue {
    if let Some(obj) = config.as_object_mut() {
        for key in PROTECTED_CONFIG_KEYS { obj.remove(*key); }
    }
    config
}

fn apply_config_patch(mut base: JsonValue, patch: JsonValue) -> JsonValue {
    if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            if !PROTECTED_CONFIG_KEYS.contains(&k.as_str()) {
                b.insert(k.clone(), v.clone());
            }
        }
    }
    base
}

// Returns true if credentials were written, false if already set (no-op).
async fn write_credentials(pool: &sqlx::PgPool, channel_id: &str, credentials: JsonValue) -> Result<bool, ApiError> {
    let conditions = credentials.as_object()
        .map(|o| o.keys().map(|k| format!("(config->>'{}') IS NULL", k)).collect::<Vec<_>>().join(" AND "))
        .unwrap_or_default();
    if conditions.is_empty() { return Ok(false); }
    let sql = format!(
        "UPDATE rootcx_system.channels SET config = config || $1::jsonb, updated_at = now()
         WHERE id = $2::uuid AND {conditions}"
    );
    let rows = sqlx::query(&sql).bind(credentials).bind(channel_id).execute(pool).await?.rows_affected();
    Ok(rows > 0)
}

fn resolve_public_url(body_url: Option<String>) -> Result<String, ApiError> {
    body_url
        .or_else(|| std::env::var("ROOTCX_PUBLIC_URL").ok())
        .ok_or_else(|| ApiError::BadRequest(
            "public_url required (pass in body or set ROOTCX_PUBLIC_URL)".into(),
        ))
}

struct PendingLink { channel_id: String, user_id: uuid::Uuid, created: Instant }

static PENDING_LINKS: LazyLock<Mutex<HashMap<String, PendingLink>>> = LazyLock::new(Default::default);
const LINK_TTL_SECS: u64 = 300;
const LINK_PREFIX: &str = "link_";

pub async fn create_link_token(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(channel_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    let (provider_name, config): (String, JsonValue) = sqlx::query_as(
        "SELECT provider, config FROM rootcx_system.channels WHERE id = $1::uuid AND status = 'active'",
    ).bind(&channel_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("channel not found".into()))?;

    if config["managed"] == true { return Err(ApiError::BadRequest("Link this channel from your authenticated SHAPP session".into())); }

    let token = format!("{LINK_PREFIX}{}", uuid::Uuid::new_v4().simple());
    PENDING_LINKS.lock().await.insert(token.clone(), PendingLink {
        channel_id: channel_id.clone(), user_id: identity.user_id, created: Instant::now(),
    });

    let mut resp = json!({ "token": token, "provider": provider_name });
    if let Some(p) = super::provider_for(&provider_name, &config) {
        if let Some(url) = p.prepare_link(&config, &token, &channel_id).await
            .map_err(|e| ApiError::Unavailable(e.to_string()))? {
            resp.as_object_mut().unwrap().insert("linkUrl".into(), json!(url));
        }
    }
    Ok(Json(resp))
}

pub async fn identity_status(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(channel_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let linked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.channel_identities
         WHERE channel_id = $1::uuid AND user_id = $2)",
    ).bind(&channel_id).bind(identity.user_id).fetch_one(&pool).await?;
    Ok(Json(json!({ "linked": linked })))
}

fn consume_link_token(
    map: &mut HashMap<String, PendingLink>, channel_id: &str, token: &str,
) -> Option<uuid::Uuid> {
    map.retain(|_, p| p.created.elapsed().as_secs() < LINK_TTL_SECS);
    let pending = map.remove(token)?;
    if pending.channel_id != channel_id { return None; }
    Some(pending.user_id)
}

async fn try_complete_link(
    pool: &sqlx::PgPool, channel_id: &str, chat_id: &str, token: &str,
) -> Option<String> {
    let provider: String = sqlx::query_scalar("SELECT provider FROM rootcx_system.channels WHERE id = $1::uuid").bind(channel_id).fetch_one(pool).await.ok()?;
    if provider == "whatsapp" { return None; }
    let user_id = consume_link_token(&mut *PENDING_LINKS.lock().await, channel_id, token)?;

    let mut tx = pool.begin().await.ok()?;
    let previous: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM rootcx_system.channel_identities WHERE channel_id = $1::uuid AND external_chat_id = $2 FOR UPDATE",
    ).bind(channel_id).bind(chat_id).fetch_optional(&mut *tx).await.ok()?;
    if let Some(previous) = previous.filter(|previous| *previous != user_id) {
        sqlx::query("DELETE FROM rootcx_system.channel_sessions WHERE channel_id = $1::uuid AND external_chat_id = $2")
            .bind(channel_id).bind(chat_id).execute(&mut *tx).await.ok()?;
        sqlx::query("UPDATE rootcx_system.delegations SET revoked_at = now() WHERE trigger_type = 'channel' AND trigger_ref = $1 AND revoked_at IS NULL")
            .bind(super::channel_delegation_ref(channel_id, previous)).execute(&mut *tx).await.ok()?;
    }
    sqlx::query(
        "INSERT INTO rootcx_system.channel_identities (channel_id, external_chat_id, user_id)
         VALUES ($1::uuid, $2, $3)
         ON CONFLICT (channel_id, external_chat_id)
         DO UPDATE SET user_id = EXCLUDED.user_id, linked_at = now()",
    ).bind(channel_id).bind(chat_id).bind(user_id)
    .execute(&mut *tx).await.ok()?;
    tx.commit().await.ok()?;

    // Phase 6b: linking grants a standing 'channel' delegation to the active
    // app's agent. The deterministic trigger_ref enables revoke_by_trigger on unlink.
    {
        let (app_id, _) = resolve_session(pool, channel_id, chat_id).await.ok()?;
        let agent_uid = crate::extensions::agents::agent_user_id(&app_id);
        let trigger_ref = super::channel_delegation_ref(channel_id, user_id);
        // Relinking must replace the former agent, not retain its trigger-scoped grant.
        crate::governance::delegation::revoke_by_trigger(pool, "channel", trigger_ref).await.ok()?;
        crate::governance::delegation::create(pool, user_id, agent_uid, "channel", Some(trigger_ref)).await.ok()?;
    }

    Some(user_id.to_string())
}

pub async fn unlink_identity(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(channel_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let trigger_ref = super::channel_delegation_ref(&channel_id, identity.user_id);

    // Atomic: delete identity + revoke delegation in one transaction
    let mut tx = pool.begin().await?;
    let deleted = sqlx::query(
        "DELETE FROM rootcx_system.channel_identities
         WHERE channel_id = $1::uuid AND user_id = $2",
    ).bind(&channel_id).bind(identity.user_id)
    .execute(&mut *tx).await?.rows_affected();

    if deleted == 0 {
        return Err(ApiError::NotFound("no linked identity for this channel".into()));
    }

    sqlx::query(
        "UPDATE rootcx_system.delegations SET revoked_at = now() \
         WHERE trigger_type = $1 AND trigger_ref = $2 AND revoked_at IS NULL"
    ).bind("channel").bind(trigger_ref).execute(&mut *tx).await?;
    tx.commit().await?;

    info!(channel_id, user_id = %identity.user_id, "channel identity unlinked, delegation revoked");
    Ok(Json(json!({ "status": "unlinked" })))
}

async fn resolve_invoker(
    pool: &sqlx::PgPool, channel_id: &str, chat_id: &str,
) -> Option<uuid::Uuid> {
    sqlx::query_scalar(
        "SELECT user_id FROM rootcx_system.channel_identities
         WHERE channel_id = $1::uuid AND external_chat_id = $2",
    ).bind(channel_id).bind(chat_id).fetch_optional(pool).await.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_patch_strips_protected_keys() {
        // apply_config_patch never writes credentials — callers must use write_credentials for that.
        let base = json!({ "team_id": "T123" });
        let patch = json!({
            "signing_secret": "attacker",
            "bot_token": "attacker",
            "webhook_secret": "attacker",
            "team_id": "T_EVIL",
        });
        let result = apply_config_patch(base, patch);
        for key in &["signing_secret", "bot_token", "webhook_secret"] {
            assert!(result[key].is_null(), "{key}: protected key must be stripped from patch");
        }
        assert_eq!(result["team_id"], "T_EVIL", "non-protected key must be patched");
    }

    #[test]
    fn redact_config_strips_secrets() {
        let cases: &[(&str, bool)] = &[
            ("bot_token", false),
            ("webhook_secret", false),
            ("signing_secret", false),
            ("other_field", true),
        ];
        for (key, should_remain) in cases {
            let config = json!({ *key: "secret_value", "unrelated": "keep" });
            let redacted = redact_config(config);
            assert_eq!(redacted[key].is_null(), !should_remain, "key '{key}' presence wrong after redact");
        }
        // unrelated key is always preserved
        let config = json!({ "bot_token": "x", "name": "my-channel" });
        let redacted = redact_config(config);
        assert_eq!(redacted["name"], "my-channel");
        assert!(redacted["bot_token"].is_null());
    }

    fn insert_token(map: &mut HashMap<String, PendingLink>, token: &str, channel_id: &str, user_id: uuid::Uuid, age: std::time::Duration) {
        map.insert(token.to_string(), PendingLink {
            channel_id: channel_id.to_string(),
            user_id,
            created: Instant::now() - age,
        });
    }

    #[test]
    fn link_token_security() {
        let uid = uuid::Uuid::new_v4();
        let ch_a = "channel-a";
        let ch_b = "channel-b";
        let cases: Vec<(&str, &str, &str, std::time::Duration, bool)> = vec![
            ("valid token, correct channel",    "tok1", ch_a, std::time::Duration::ZERO,          true),
            ("wrong channel",                   "tok2", ch_b, std::time::Duration::ZERO,          false),
            ("expired token",                   "tok3", ch_a, std::time::Duration::from_secs(301), false),
            ("nonexistent token",               "tok_missing", ch_a, std::time::Duration::ZERO,   false),
            ("replay: token consumed",          "tok1", ch_a, std::time::Duration::ZERO,          false),
        ];

        let mut map = HashMap::new();
        insert_token(&mut map, "tok1", ch_a, uid, std::time::Duration::ZERO);
        insert_token(&mut map, "tok2", ch_a, uid, std::time::Duration::ZERO); // scoped to ch_a
        insert_token(&mut map, "tok3", ch_a, uid, std::time::Duration::from_secs(301));

        for (desc, token, channel, _, expect) in &cases {
            let result = consume_link_token(&mut map, channel, token);
            assert_eq!(result.is_some(), *expect, "{desc}: expected {expect}, got {}", result.is_some());
            if *expect { assert_eq!(result.unwrap(), uid, "{desc}: wrong user_id"); }
        }
    }
}
