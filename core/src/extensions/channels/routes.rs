use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use tokio::sync::Mutex;
use tokio::task::AbortHandle;
use tracing::{error, info};

use super::types::{ChannelProvider, InboundEvent};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::agents::{self, config as agent_config, persistence};
use crate::extensions::agents::approvals::ApprovalResponse;
use crate::ipc::{AgentInvokePayload, LlmModelRef};
use crate::routes::{self, SharedRuntime, llm_models::fetch_default_llm};
use crate::worker_manager::WorkerManager;

#[derive(Deserialize)]
pub struct CreateChannel {
    provider: String,
    name: String,
    config: JsonValue,
}

pub async fn create_channel(
    _identity: Identity, State(rt): State<SharedRuntime>, Json(body): Json<CreateChannel>,
) -> Result<Json<JsonValue>, ApiError> {
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

    Ok(Json(rows.into_iter().map(|(id, provider, name, mut config, status, ca, ua)| {
        if let Some(obj) = config.as_object_mut() {
            obj.remove("bot_token");
            obj.remove("webhook_secret");
        }
        json!({ "id": id, "provider": provider, "name": name, "config": config, "status": status, "createdAt": ca, "updatedAt": ua })
    }).collect()))
}

pub async fn delete_channel(
    _identity: Identity, State(rt): State<SharedRuntime>, Path(channel_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    if let Some((prov, cfg)) = sqlx::query_as::<_, (String, JsonValue)>(
        "SELECT provider, config FROM rootcx_system.channels WHERE id = $1::uuid",
    ).bind(&channel_id).fetch_optional(&pool).await? {
        if let Some(p) = super::provider(&prov) { let _ = p.unregister_webhook(&cfg).await; }
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
pub struct ActivateChannel { pub public_url: Option<String> }

pub async fn activate_channel(
    _identity: Identity, State(rt): State<SharedRuntime>, Path(channel_id): Path<String>,
    body: Option<Json<ActivateChannel>>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let (prov, cfg) = load_channel(sqlx::query_as(
        "SELECT provider, config FROM rootcx_system.channels WHERE id = $1::uuid",
    ).bind(&channel_id).fetch_optional(&pool).await?, &channel_id)?;

    let provider = super::provider(&prov)
        .ok_or_else(|| ApiError::Internal(format!("unknown provider: {prov}")))?;
    let base = resolve_public_url(body.and_then(|b| b.0.public_url))?;
    let url = format!("{base}/api/v1/channels/{prov}/{channel_id}/webhook");
    provider.register_webhook(&cfg, &url).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    sqlx::query("UPDATE rootcx_system.channels SET status = 'active', updated_at = now() WHERE id = $1::uuid")
        .bind(&channel_id).execute(&pool).await?;
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

    if let Some(p) = super::provider(&prov) { let _ = p.unregister_webhook(&cfg).await; }
    sqlx::query("UPDATE rootcx_system.channels SET status = 'inactive', updated_at = now() WHERE id = $1::uuid")
        .bind(&channel_id).execute(&pool).await?;
    info!(channel_id, "channel deactivated");
    Ok(Json(json!({ "status": "inactive" })))
}

struct DebounceEntry {
    texts: Vec<String>,
    abort: Option<AbortHandle>,
}

static DEBOUNCE: LazyLock<Mutex<HashMap<(String, String), DebounceEntry>>> =
    LazyLock::new(Default::default);

pub async fn webhook(
    State(rt): State<SharedRuntime>,
    Path((provider_name, channel_id)): Path<(String, String)>,
    headers: HeaderMap, body: Bytes,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = rt.pool().clone();

    let (config, status): (JsonValue, String) = sqlx::query_as(
        "SELECT config, status FROM rootcx_system.channels WHERE id = $1::uuid AND provider = $2",
    ).bind(&channel_id).bind(&provider_name).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("channel not found".into()))?;

    if status != "active" { return Ok(Json(json!({ "ok": true }))); }

    let provider = super::provider(&provider_name)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown provider: {provider_name}")))?;
    let event = provider.parse_webhook(&config, body, &headers).await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    match event {
        InboundEvent::Ignored => {}
        InboundEvent::Callback { chat_id, callback_id, data } => {
            handle_callback(&rt, &channel_id, &config, provider.as_ref(), &chat_id, &callback_id, &data).await;
        }
        InboundEvent::Message { chat_id, text } => {
            if handle_command(&pool, &channel_id, &chat_id, &text, &config, provider.as_ref()).await {
                return Ok(Json(json!({ "ok": true })));
            }

            let needs_debounce = provider.debounce_ms().filter(|_| text.len() >= 4096);
            if let Some(ms) = needs_debounce {
                debounce_and_invoke(rt, channel_id, provider_name, config, chat_id, text, ms);
            } else {
                flush_and_invoke(rt, channel_id, provider_name, config, chat_id, text);
            }
        }
    }

    Ok(Json(json!({ "ok": true })))
}

fn debounce_and_invoke(
    rt: SharedRuntime, channel_id: String, provider_name: String,
    config: JsonValue, chat_id: String, text: String, ms: u64,
) {
    let key = (channel_id.clone(), chat_id.clone());

    tokio::spawn(async move {
        let mut map = DEBOUNCE.lock().await;
        if let Some(entry) = map.get_mut(&key) {
            if let Some(h) = entry.abort.take() { h.abort(); }
            entry.texts.push(text);
        } else {
            map.insert(key.clone(), DebounceEntry { texts: vec![text], abort: None });
        }

        let timer_key = key.clone();
        let timer_rt = rt.clone();
        let timer_prov = provider_name.clone();
        let timer_cfg = config.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            let combined = {
                let mut map = DEBOUNCE.lock().await;
                map.remove(&timer_key).map(|e| e.texts.join("\n")).unwrap_or_default()
            };
            if !combined.is_empty() {
                dispatch_invoke(timer_rt, timer_key.0, timer_cfg, timer_prov, timer_key.1, combined);
            }
        });

        map.get_mut(&key).unwrap().abort = Some(handle.abort_handle());
    });
}

fn flush_and_invoke(
    rt: SharedRuntime, channel_id: String, provider_name: String,
    config: JsonValue, chat_id: String, text: String,
) {
    let key = (channel_id.clone(), chat_id.clone());
    tokio::spawn(async move {
        let prefix = {
            let mut map = DEBOUNCE.lock().await;
            if let Some(entry) = map.remove(&key) {
                if let Some(h) = entry.abort { h.abort(); }
                Some(entry.texts.join("\n"))
            } else {
                None
            }
        };
        let combined = match prefix {
            Some(mut p) => { p.push('\n'); p.push_str(&text); p }
            None => text,
        };
        dispatch_invoke(rt, channel_id, config, provider_name, chat_id, combined);
    });
}

fn dispatch_invoke(
    rt: SharedRuntime, channel_id: String, config: JsonValue,
    provider_name: String, chat_id: String, text: String,
) {
    let pool = rt.pool().clone();
    let wm = rt.worker_manager().clone();

    tokio::spawn(async move {
        if let Err(e) = do_invoke(&pool, &wm, &channel_id, &config, &provider_name, &chat_id, &text).await {
            error!(channel_id, chat_id, "invoke failed: {e}");
        }
    });
}

async fn do_invoke(
    pool: &sqlx::PgPool, wm: &Arc<WorkerManager>,
    channel_id: &str, config: &JsonValue, provider_name: &str,
    chat_id: &str, text: &str,
) -> Result<(), String> {
    fn e(e: impl std::fmt::Debug) -> String { format!("{e:?}") }
    let (app_id, session_id) = resolve_session(pool, channel_id, chat_id).await.map_err(&e)?;

    let agent_cfg: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
    ).bind(&app_id).fetch_one(pool).await.map_err(&e)?;
    let memory = agent_cfg.pointer("/memory/enabled").and_then(JsonValue::as_bool) == Some(true);
    let history = agent_config::load_history(pool, memory, &app_id, &session_id).await.map_err(&e)?;
    let llm = fetch_default_llm(pool).await.map_err(&e)?
        .map(|(p, m)| LlmModelRef { provider: p, model: m });

    let payload = AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        message: text.to_string(),
        history, is_sub_invoke: false, llm,
    };

    let mut rx = wm.agent_invoke(&app_id, payload).await.map_err(&e)?;
    let provider = super::provider(provider_name).unwrap();
    let typing = provider.start_typing(config, chat_id);

    let mut response = String::new();
    let mut tokens = None;
    while let Some(event) = rx.recv().await {
        match event {
            crate::worker::AgentEvent::Chunk { delta } => response.push_str(&delta),
            crate::worker::AgentEvent::Done { response: r, tokens: t } => { response = r; tokens = t; break; }
            crate::worker::AgentEvent::ApprovalRequired { approval_id, tool_name, args, .. } => {
                let _ = provider.send_approval(config, chat_id, &approval_id, &tool_name, &args).await;
            }
            crate::worker::AgentEvent::Error { error: e } => {
                if let Some(h) = &typing { h.abort(); }
                error!(channel_id, chat_id, "agent error: {e}");
                let _ = provider.send_response(config, chat_id, &format!("Error: {e}")).await;
                return Ok(());
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
    }
    Ok(())
}

const DEFAULT_AGENT: &str = "assistant";

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
    let session_id = create_session(pool, channel_id, chat_id, DEFAULT_AGENT).await?;
    Ok((DEFAULT_AGENT.to_string(), session_id))
}

async fn handle_callback(
    rt: &SharedRuntime, channel_id: &str, config: &JsonValue, provider: &dyn ChannelProvider,
    chat_id: &str, callback_id: &str, data: &str,
) {
    let (action, payload) = data.split_once(':').unwrap_or(("", ""));

    match action {
        "approve" => {
            handle_approval(rt, config, provider, chat_id, callback_id, payload, ApprovalResponse::Approved, "Approved").await;
        }
        "deny" => {
            handle_approval(rt, config, provider, chat_id, callback_id, payload,
                ApprovalResponse::Rejected { reason: "rejected via chat".into() }, "Denied").await;
        }
        "agent" => {
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

async fn handle_approval(
    rt: &SharedRuntime, config: &JsonValue, provider: &dyn ChannelProvider,
    chat_id: &str, callback_id: &str, approval_id: &str,
    response: ApprovalResponse, ack: &str,
) {
    let replied = rt.pending_approvals().reply(approval_id, response).await;
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
    let parts: Vec<&str> = text.split_whitespace().collect();
    let Some(cmd) = parts.first().and_then(|p| p.split('@').next()) else { return false };
    match cmd {
        "/newsession" => {
            let _ = sqlx::query(
                "DELETE FROM rootcx_system.channel_sessions
                 WHERE channel_id = $1::uuid AND external_chat_id = $2",
            ).bind(channel_id).bind(chat_id).execute(pool).await;
            let _ = provider.send_response(config, chat_id, "New session started.").await;
            true
        }
        "/start" => {
            let _ = provider.send_response(config, chat_id, "Send me a message.").await;
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
                    .map(|(id, name)| (name, format!("agent:{id}")))
                    .collect();
                let _ = provider.send_choice(config, chat_id, "Choose an agent:", &options).await;
            }
            true
        }
        _ => false,
    }
}

fn resolve_public_url(body_url: Option<String>) -> Result<String, ApiError> {
    body_url
        .or_else(|| std::env::var("ROOTCX_PUBLIC_URL").ok())
        .ok_or_else(|| ApiError::BadRequest(
            "public_url required (pass in body or set ROOTCX_PUBLIC_URL)".into(),
        ))
}
