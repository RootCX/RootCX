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

use super::types::ChannelBinding;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::agents::{self, config as agent_config, persistence};
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

#[derive(Deserialize)]
pub struct BindAgent { app_id: String, #[serde(default)] routing: Option<JsonValue> }

pub async fn bind_agent(
    _identity: Identity, State(rt): State<SharedRuntime>,
    Path(channel_id): Path<String>, Json(body): Json<BindAgent>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id = $1)",
    ).bind(&body.app_id).fetch_one(&pool).await?;
    if !exists { return Err(ApiError::NotFound(format!("agent '{}' not found", body.app_id))); }

    sqlx::query(
        "INSERT INTO rootcx_system.channel_bindings (channel_id, app_id, routing)
         VALUES ($1::uuid, $2, $3)
         ON CONFLICT (channel_id, app_id) DO UPDATE SET routing = EXCLUDED.routing",
    ).bind(&channel_id).bind(&body.app_id).bind(&body.routing)
    .execute(&pool).await?;
    info!(channel_id, app_id = %body.app_id, "agent bound to channel");
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn unbind_agent(
    _identity: Identity, State(rt): State<SharedRuntime>,
    Path((channel_id, app_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    sqlx::query("DELETE FROM rootcx_system.channel_bindings WHERE channel_id = $1::uuid AND app_id = $2")
        .bind(&channel_id).bind(&app_id).execute(&routes::pool(&rt)).await?;
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn list_bindings(
    _identity: Identity, State(rt): State<SharedRuntime>, Path(channel_id): Path<String>,
) -> Result<Json<Vec<ChannelBinding>>, ApiError> {
    Ok(Json(sqlx::query_as::<_, ChannelBinding>(
        "SELECT channel_id::text, app_id, routing FROM rootcx_system.channel_bindings WHERE channel_id = $1::uuid",
    ).bind(&channel_id).fetch_all(&routes::pool(&rt)).await?))
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
    let inbound = provider.parse_webhook(&config, body, &headers).await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    // Only debounce when the message hits the provider's char limit (likely split)
    let needs_debounce = provider.debounce_ms()
        .filter(|_| inbound.text.len() >= 4096);

    if let Some(ms) = needs_debounce {
        debounce_and_invoke(rt, channel_id, provider_name, config, inbound.chat_id, inbound.text, ms);
    } else {
        // Flush any pending debounce buffer then dispatch with this message appended
        flush_and_invoke(rt, channel_id, provider_name, config, inbound.chat_id, inbound.text);
    }

    Ok(Json(json!({ "ok": true })))
}

/// Buffer messages per (channel, chat) and invoke after `ms` of silence.
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

/// Message < 4096: likely final or standalone. Flush any pending buffer and invoke.
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
    let app_id = resolve_agent(pool, channel_id).await.map_err(&e)?;
    let session_id = get_or_create_session(pool, channel_id, chat_id, &app_id).await.map_err(&e)?;

    let agent_cfg: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
    ).bind(&app_id).fetch_one(pool).await.map_err(&e)?;
    let memory = agent_cfg.pointer("/memory/enabled").and_then(serde_json::Value::as_bool) == Some(true);
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

    let mut response = String::new();
    let mut tokens = None;
    while let Some(event) = rx.recv().await {
        match event {
            crate::worker::AgentEvent::Chunk { delta } => response.push_str(&delta),
            crate::worker::AgentEvent::Done { response: r, tokens: t } => { response = r; tokens = t; break; }
            crate::worker::AgentEvent::Error { error: e } => {
                error!(channel_id, chat_id, "agent error: {e}");
                let _ = provider.send_response(config, chat_id, &format!("Error: {e}")).await;
                return Ok(());
            }
            _ => {}
        }
    }
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

async fn resolve_agent(pool: &sqlx::PgPool, channel_id: &str) -> Result<String, ApiError> {
    sqlx::query_scalar(
        "SELECT app_id FROM rootcx_system.channel_bindings WHERE channel_id = $1::uuid LIMIT 1",
    ).bind(channel_id).fetch_optional(pool).await?
    .ok_or_else(|| ApiError::BadRequest("no agent bound to this channel".into()))
}

async fn get_or_create_session(
    pool: &sqlx::PgPool, channel_id: &str, chat_id: &str, app_id: &str,
) -> Result<String, ApiError> {
    if let Some(sid) = sqlx::query_scalar::<_, String>(
        "SELECT session_id::text FROM rootcx_system.channel_sessions
         WHERE channel_id = $1::uuid AND external_chat_id = $2",
    ).bind(channel_id).bind(chat_id).fetch_optional(pool).await? {
        return Ok(sid);
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    persistence::ensure_session(pool, &session_id, app_id, agents::agent_user_id(app_id)).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    sqlx::query(
        "INSERT INTO rootcx_system.channel_sessions (channel_id, external_chat_id, app_id, session_id)
         VALUES ($1::uuid, $2, $3, $4::uuid) ON CONFLICT (channel_id, external_chat_id) DO NOTHING",
    ).bind(channel_id).bind(chat_id).bind(app_id).bind(&session_id)
    .execute(pool).await?;
    Ok(session_id)
}

fn resolve_public_url(body_url: Option<String>) -> Result<String, ApiError> {
    body_url
        .or_else(|| std::env::var("ROOTCX_PUBLIC_URL").ok())
        .ok_or_else(|| ApiError::BadRequest(
            "public_url required (pass in body or set ROOTCX_PUBLIC_URL)".into(),
        ))
}
