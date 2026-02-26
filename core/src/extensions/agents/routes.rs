use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::error;

use uuid::Uuid;

use super::agent_user_id;
use crate::api_error::ApiError;
use crate::auth::jwt;
use crate::auth::identity::Identity;
use crate::ipc::{AgentInvokePayload, RpcCaller};
use crate::routes::{self, SharedRuntime};
use crate::secrets::SecretManager;
use crate::worker::AgentEvent;

#[derive(Deserialize)]
pub struct InvokeRequest {
    pub message: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct AgentRow {
    app_id: String,
    name: String,
    description: Option<String>,
    config: JsonValue,
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct SessionRow {
    id: String,
    messages: JsonValue,
    created_at: String,
    updated_at: String,
}

struct PersistCtx {
    pool: PgPool,
    app_id: String,
    user_id: Uuid,
    user_message: String,
}

pub async fn get_agent(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<AgentRow>, ApiError> {
    let pool = routes::pool(&rt).await?;
    sqlx::query_as::<_, AgentRow>(
        "SELECT app_id, name, description, config
         FROM rootcx_system.agents WHERE app_id = $1",
    )
    .bind(&app_id)
    .fetch_optional(&pool)
    .await?
    .map(Json)
    .ok_or_else(|| ApiError::NotFound(format!("no agent for app '{app_id}'")))
}

pub async fn invoke_agent(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<InvokeRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (pool, wm, data_dir, auth_cfg, sm, tool_registry) = {
        let g = rt.lock().await;
        (
            g.pool().cloned().ok_or(ApiError::NotReady)?,
            g.worker_manager().cloned().ok_or(ApiError::NotReady)?,
            g.data_dir().to_path_buf(),
            g.auth_config().clone(),
            g.secret_manager().cloned().ok_or(ApiError::NotReady)?,
            g.tool_registry().clone(),
        )
    };

    let config: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
    )
    .bind(&app_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("no agent for app '{app_id}'")))?;

    let session_id = body
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let memory_enabled = config
        .get("memory")
        .and_then(|m| m.get("enabled"))
        .and_then(|e| e.as_bool())
        == Some(true);

    let history = load_history(&pool, memory_enabled, &session_id).await?;
    let system_prompt = load_system_prompt(&app_id, &config, &data_dir).await?;
    let agent_config = build_agent_config(&pool, &sm, &app_id, &config, &tool_registry).await?;

    let agent_token = jwt::encode_access(&auth_cfg, agent_user_id(&app_id), &format!("agent:{app_id}"))
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let caller = Some(RpcCaller {
        user_id: identity.user_id.to_string(),
        username: identity.username.clone(),
    });

    let persist_ctx = if memory_enabled {
        Some(PersistCtx {
            pool: pool.clone(),
            app_id: app_id.clone(),
            user_id: identity.user_id,
            user_message: body.message.clone(),
        })
    } else {
        None
    };

    let payload = AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        message: body.message,
        system_prompt,
        config: agent_config,
        auth_token: agent_token,
        history,
        caller,
    };

    let stream_rx = wm.agent_invoke(&app_id, payload).await?;
    let session_id: Arc<str> = session_id.into();

    Ok(Sse::new(build_sse_stream(stream_rx, session_id, persist_ctx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

fn build_sse_stream(
    stream_rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    session_id: Arc<str>,
    persist_ctx: Option<PersistCtx>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures::stream::unfold(
        (stream_rx, persist_ctx),
        move |(mut rx, ctx)| {
            let sid = Arc::clone(&session_id);
            async move {
                match rx.recv().await {
                    Some(AgentEvent::Chunk { delta }) => {
                        let event = Event::default()
                            .event("chunk")
                            .data(json!({"delta": delta, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::Done { response, tokens }) => {
                        if let Some(ref pctx) = ctx {
                            if let Err(e) = persist_session(&pctx.pool, &sid, &pctx.app_id, pctx.user_id, &pctx.user_message, &response).await {
                                error!(session_id = %sid, "failed to persist session: {e}");
                            }
                        }
                        let event = Event::default()
                            .event("done")
                            .data(json!({"response": response, "session_id": &*sid, "tokens": tokens}).to_string());
                        Some((Ok(event), (rx, None)))
                    }
                    Some(AgentEvent::Error { error }) => {
                        let event = Event::default()
                            .event("error")
                            .data(json!({"error": error, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, None)))
                    }
                    None => None,
                }
            }
        },
    )
}

async fn load_history(
    pool: &PgPool,
    memory_enabled: bool,
    session_id: &str,
) -> Result<Vec<JsonValue>, ApiError> {
    if !memory_enabled {
        return Ok(vec![]);
    }
    match sqlx::query_scalar::<_, JsonValue>(
        "SELECT messages FROM rootcx_system.agent_sessions WHERE id = $1::uuid",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    {
        None => Ok(vec![]),
        Some(v) => v
            .as_array()
            .cloned()
            .ok_or_else(|| ApiError::Internal("agent session messages is not a JSON array".into())),
    }
}

async fn build_agent_config(
    pool: &PgPool,
    sm: &SecretManager,
    app_id: &str,
    config: &JsonValue,
    tool_registry: &crate::tools::ToolRegistry,
) -> Result<JsonValue, ApiError> {
    let enabled_tools = extract_enabled_tools(config);
    let data_contract: JsonValue = sqlx::query_scalar(
        "SELECT COALESCE(manifest->'dataContract', '[]'::jsonb) FROM rootcx_system.apps WHERE id = $1",
    )
    .bind(app_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("app '{app_id}' not found")))?;

    let tool_descriptors = tool_registry.descriptors_for(&enabled_tools, &data_contract);

    let mut provider = config.get("provider").cloned().unwrap_or(json!(null));
    resolve_secret_refs(pool, sm, &mut provider).await?;

    Ok(json!({
        "provider": provider,
        "limits": config.get("limits"),
        "_appId": app_id,
        "_toolDescriptors": tool_descriptors,
        "_graphPath": config.get("graph"),
    }))
}

async fn resolve_secret_refs(
    pool: &PgPool,
    sm: &SecretManager,
    value: &mut JsonValue,
) -> Result<(), ApiError> {
    let Some(obj) = value.as_object() else { return Ok(()) };
    let mut resolved = Vec::new();
    for (k, v) in obj {
        let Some(raw) = v.as_str() else { continue };
        if let Some(key) = raw.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
            let secret = sm
                .get(pool, "_platform", key)
                .await
                .map_err(|e| ApiError::Internal(format!("secret resolution failed: {e}")))?
                .ok_or_else(|| ApiError::BadRequest(format!("platform secret '{key}' not found")))?;
            resolved.push((k.clone(), secret));
        }
    }
    let obj = value.as_object_mut().unwrap();
    for (k, secret) in resolved {
        obj.insert(k, JsonValue::String(secret));
    }
    Ok(())
}

pub async fn list_sessions(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows = sqlx::query_as::<_, SessionRow>(
        "SELECT id::text, messages, created_at::text, updated_at::text
         FROM rootcx_system.agent_sessions
         WHERE app_id = $1
         ORDER BY updated_at DESC",
    )
    .bind(&app_id)
    .fetch_all(&pool)
    .await?;
    Ok(Json(rows))
}

pub async fn get_session(
    State(rt): State<SharedRuntime>,
    Path((app_id, session_id)): Path<(String, String)>,
) -> Result<Json<SessionRow>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let row = sqlx::query_as::<_, SessionRow>(
        "SELECT id::text, messages, created_at::text, updated_at::text
         FROM rootcx_system.agent_sessions
         WHERE app_id = $1 AND id = $2::uuid",
    )
    .bind(&app_id)
    .bind(&session_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("session '{session_id}' not found")))?;
    Ok(Json(row))
}

fn extract_enabled_tools(config: &JsonValue) -> Vec<String> {
    let mut tools = vec!["list_apps".into(), "describe_app".into()];
    let mut has_data_access = false;
    let Some(entries) = config.get("access").and_then(|a| a.as_array()) else {
        return tools;
    };
    for entry in entries {
        let Some(entity) = entry.get("entity").and_then(|e| e.as_str()) else { continue };
        if let Some(tool_name) = entity.strip_prefix("tool:") {
            tools.push(tool_name.to_string());
        } else {
            has_data_access = true;
        }
    }
    if has_data_access {
        tools.push("query_data".into());
        tools.push("mutate_data".into());
    }
    tools
}

async fn load_system_prompt(
    app_id: &str,
    config: &JsonValue,
    data_dir: &std::path::Path,
) -> Result<String, ApiError> {
    let prompt_path = config
        .get("systemPrompt")
        .and_then(|p| p.as_str())
        .unwrap_or("./agent/system.md");
    let rel = prompt_path.strip_prefix("./").unwrap_or(prompt_path);
    let full_path = data_dir.join("apps").join(app_id).join(rel);
    tokio::fs::read_to_string(&full_path).await.map_err(|e| {
        ApiError::Internal(format!("failed to read system prompt at {}: {e}", full_path.display()))
    })
}

async fn persist_session(
    pool: &PgPool,
    session_id: &str,
    app_id: &str,
    user_id: Uuid,
    user_message: &str,
    assistant_response: &str,
) -> Result<(), sqlx::Error> {
    let new_messages = json!([
        {"role": "user", "content": user_message},
        {"role": "assistant", "content": assistant_response}
    ]);
    sqlx::query(
        "INSERT INTO rootcx_system.agent_sessions (id, app_id, user_id, messages)
         VALUES ($1::uuid, $2, $3, $4)
         ON CONFLICT (id) DO UPDATE SET
             messages = rootcx_system.agent_sessions.messages || $4,
             updated_at = now()",
    )
    .bind(session_id)
    .bind(app_id)
    .bind(user_id)
    .bind(&new_messages)
    .execute(pool)
    .await?;
    Ok(())
}
