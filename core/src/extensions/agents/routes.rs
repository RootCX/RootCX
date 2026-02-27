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
use super::approvals::{ApprovalReply, ApprovalResponse, PendingApprovals};
use crate::api_error::ApiError;
use crate::auth::jwt;
use crate::auth::identity::Identity;
use crate::ipc::{AgentInvokePayload, RpcCaller};
use crate::routes::{self, SharedRuntime};
use crate::secrets::SecretManager;
use crate::worker::AgentEvent;

static APPROVALS: std::sync::OnceLock<PendingApprovals> = std::sync::OnceLock::new();

fn approvals() -> &'static PendingApprovals {
    APPROVALS.get_or_init(PendingApprovals::new)
}

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
    #[sqlx(default)]
    title: Option<String>,
    #[sqlx(default)]
    status: Option<String>,
    #[sqlx(default)]
    total_tokens: Option<i64>,
    #[sqlx(default)]
    turn_count: Option<i32>,
}

#[derive(Serialize, sqlx::FromRow)]
struct MessageRow {
    id: String,
    role: String,
    content: Option<String>,
    token_count: Option<i32>,
    is_summary: bool,
    created_at: String,
}

#[derive(Serialize, sqlx::FromRow)]
struct ToolCallRow {
    id: String,
    tool_name: String,
    input: JsonValue,
    output: Option<JsonValue>,
    error: Option<String>,
    status: String,
    duration_ms: Option<i32>,
    created_at: String,
}

#[derive(Serialize)]
pub(crate) struct SessionEventEntry {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(flatten)]
    data: JsonValue,
}

struct PersistCtx {
    pool: PgPool,
    app_id: String,
    session_id: String,
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
            session_id: session_id.clone(),
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
                            if let Err(e) = persist_session(pctx, &response, tokens).await {
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
                    Some(AgentEvent::ToolCallStarted { call_id, tool_name, input }) => {
                        if let Some(ref pctx) = ctx {
                            let _ = persist_tool_call_start(&pctx.pool, &pctx.session_id, &call_id, &tool_name, &input).await;
                        }
                        let event = Event::default()
                            .event("tool_call_started")
                            .data(json!({"call_id": call_id, "tool_name": tool_name, "input": input, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::ToolCallCompleted { call_id, tool_name, output, error, duration_ms }) => {
                        if let Some(ref pctx) = ctx {
                            let _ = persist_tool_call_end(&pctx.pool, &call_id, output.as_ref(), error.as_deref(), duration_ms).await;
                        }
                        let event = Event::default()
                            .event("tool_call_completed")
                            .data(json!({
                                "call_id": call_id, "tool_name": tool_name,
                                "output": output, "error": error,
                                "duration_ms": duration_ms, "session_id": &*sid
                            }).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::ApprovalRequired { approval_id, tool_name, args, reason }) => {
                        let event = Event::default()
                            .event("approval_required")
                            .data(json!({
                                "approval_id": approval_id, "tool_name": tool_name,
                                "args": args, "reason": reason, "session_id": &*sid
                            }).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::SessionCompacted { summary }) => {
                        if let Some(ref pctx) = ctx {
                            let _ = persist_message(&pctx.pool, &pctx.session_id, "system", &summary, None, true).await;
                        }
                        let event = Event::default()
                            .event("session_compacted")
                            .data(json!({"summary": summary, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    None => None,
                }
            }
        },
    )
}

pub async fn list_sessions(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows = sqlx::query_as::<_, SessionRow>(
        "SELECT id::text, messages, created_at::text, updated_at::text,
                title, status, total_tokens, turn_count
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
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let session = sqlx::query_as::<_, SessionRow>(
        "SELECT id::text, messages, created_at::text, updated_at::text,
                title, status, total_tokens, turn_count
         FROM rootcx_system.agent_sessions
         WHERE app_id = $1 AND id = $2::uuid",
    )
    .bind(&app_id)
    .bind(&session_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("session '{session_id}' not found")))?;

    let messages = sqlx::query_as::<_, MessageRow>(
        "SELECT id::text, role, content, token_count, is_summary, created_at::text
         FROM rootcx_system.agent_messages
         WHERE session_id = $1::uuid
         ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&pool)
    .await?;

    let response = if messages.is_empty() {
        json!({
            "id": session.id,
            "title": session.title,
            "status": session.status,
            "totalTokens": session.total_tokens,
            "turnCount": session.turn_count,
            "messages": session.messages,
            "createdAt": session.created_at,
            "updatedAt": session.updated_at,
        })
    } else {
        let tool_calls = sqlx::query_as::<_, ToolCallRow>(
            "SELECT id::text, tool_name, input, output, error, status, duration_ms, created_at::text
             FROM rootcx_system.agent_tool_calls
             WHERE session_id = $1::uuid
             ORDER BY created_at ASC",
        )
        .bind(&session_id)
        .fetch_all(&pool)
        .await?;

        let structured_messages: Vec<JsonValue> = messages.iter().map(|m| {
            let mut msg = json!({
                "id": m.id,
                "role": m.role,
                "content": m.content,
                "tokenCount": m.token_count,
                "isSummary": m.is_summary,
                "createdAt": m.created_at,
            });
            if m.role == "assistant" {
                let msg_tools: Vec<&ToolCallRow> = tool_calls.iter()
                    .filter(|tc| tc.created_at >= m.created_at)
                    .collect();
                if !msg_tools.is_empty() {
                    msg["toolCalls"] = json!(msg_tools);
                }
            }
            msg
        }).collect();

        json!({
            "id": session.id,
            "title": session.title,
            "status": session.status,
            "totalTokens": session.total_tokens,
            "turnCount": session.turn_count,
            "messages": structured_messages,
            "createdAt": session.created_at,
            "updatedAt": session.updated_at,
        })
    };

    Ok(Json(response))
}

pub async fn get_session_events(
    State(rt): State<SharedRuntime>,
    Path((_app_id, session_id)): Path<(String, String)>,
) -> Result<Json<Vec<SessionEventEntry>>, ApiError> {
    let pool = routes::pool(&rt).await?;

    let messages = sqlx::query_as::<_, MessageRow>(
        "SELECT id::text, role, content, token_count, is_summary, created_at::text
         FROM rootcx_system.agent_messages
         WHERE session_id = $1::uuid ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&pool)
    .await?;

    let tool_calls = sqlx::query_as::<_, ToolCallRow>(
        "SELECT id::text, tool_name, input, output, error, status, duration_ms, created_at::text
         FROM rootcx_system.agent_tool_calls
         WHERE session_id = $1::uuid ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&pool)
    .await?;

    let mut events: Vec<SessionEventEntry> = Vec::new();

    for m in &messages {
        events.push(SessionEventEntry {
            event_type: "message".into(),
            data: json!({
                "id": m.id, "role": m.role, "content": m.content,
                "tokenCount": m.token_count, "isSummary": m.is_summary,
                "at": m.created_at,
            }),
        });
    }

    for tc in &tool_calls {
        events.push(SessionEventEntry {
            event_type: "tool_call".into(),
            data: json!({
                "id": tc.id, "toolName": tc.tool_name, "input": tc.input,
                "output": tc.output, "error": tc.error, "status": tc.status,
                "durationMs": tc.duration_ms, "at": tc.created_at,
            }),
        });
    }

    events.sort_by(|a, b| {
        let a_at = a.data.get("at").and_then(|v| v.as_str()).unwrap_or("");
        let b_at = b.data.get("at").and_then(|v| v.as_str()).unwrap_or("");
        a_at.cmp(b_at)
    });

    Ok(Json(events))
}

pub async fn list_approvals(
    State(_rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<super::approvals::ApprovalRequest>>, ApiError> {
    Ok(Json(approvals().list(&app_id).await))
}

pub async fn reply_approval(
    State(_rt): State<SharedRuntime>,
    Path((_app_id, approval_id)): Path<(String, String)>,
    Json(body): Json<ApprovalReply>,
) -> Result<Json<JsonValue>, ApiError> {
    let response = match body.action {
        super::approvals::ApprovalAction::Approve => ApprovalResponse::Approved,
        super::approvals::ApprovalAction::Reject => ApprovalResponse::Rejected {
            reason: body.reason.unwrap_or_else(|| "rejected by user".into()),
        },
    };
    let found = approvals().reply(&approval_id, response).await;
    if found {
        Ok(Json(json!({"status": "ok"})))
    } else {
        Err(ApiError::NotFound(format!("approval '{approval_id}' not found")))
    }
}

async fn load_history(
    pool: &PgPool,
    memory_enabled: bool,
    session_id: &str,
) -> Result<Vec<JsonValue>, ApiError> {
    if !memory_enabled {
        return Ok(vec![]);
    }

    let messages: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT role, content FROM rootcx_system.agent_messages
         WHERE session_id = $1::uuid ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;

    if !messages.is_empty() {
        return Ok(messages.into_iter().map(|(role, content)| {
            json!({"role": role, "content": content.unwrap_or_default()})
        }).collect());
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
    let enabled_tools = extract_enabled_tools(app_id, config, pool).await?;
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
        "_supervision": config.get("supervision"),
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

async fn extract_enabled_tools(app_id: &str, config: &JsonValue, pool: &PgPool) -> Result<Vec<String>, ApiError> {
    let mut tools = vec!["list_apps".into(), "describe_app".into()];
    let mut has_data_access = false;

    if let Some(entries) = config.get("access").and_then(|a| a.as_array()) {
        for entry in entries {
            let Some(entity) = entry.get("entity").and_then(|e| e.as_str()) else { continue };
            if let Some(tool_name) = entity.strip_prefix("tool:") {
                tools.push(tool_name.to_string());
            } else {
                has_data_access = true;
            }
        }
    }

    if !has_data_access {
        let has_entities: bool = sqlx::query_scalar(
            "SELECT jsonb_array_length(COALESCE(manifest->'dataContract', '[]'::jsonb)) > 0
             FROM rootcx_system.apps WHERE id = $1",
        )
        .bind(app_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(false);

        if has_entities {
            has_data_access = true;
        }
    }

    if has_data_access {
        tools.push("query_data".into());
        tools.push("mutate_data".into());
    }
    Ok(tools)
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

async fn ensure_session(pool: &PgPool, session_id: &str, app_id: &str, user_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO rootcx_system.agent_sessions (id, app_id, user_id)
         VALUES ($1::uuid, $2, $3)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(session_id)
    .bind(app_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn persist_message(
    pool: &PgPool,
    session_id: &str,
    role: &str,
    content: &str,
    token_count: Option<i32>,
    is_summary: bool,
) -> Result<Uuid, sqlx::Error> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.agent_messages (session_id, role, content, token_count, is_summary)
         VALUES ($1::uuid, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(session_id)
    .bind(role)
    .bind(content)
    .bind(token_count.unwrap_or(0))
    .bind(is_summary)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn persist_tool_call_start(
    pool: &PgPool,
    session_id: &str,
    call_id: &str,
    tool_name: &str,
    input: &JsonValue,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO rootcx_system.agent_tool_calls (id, session_id, tool_name, input, status)
         VALUES ($1::uuid, $2::uuid, $3, $4, 'running')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(call_id)
    .bind(session_id)
    .bind(tool_name)
    .bind(input)
    .execute(pool)
    .await?;
    Ok(())
}

async fn persist_tool_call_end(
    pool: &PgPool,
    call_id: &str,
    output: Option<&JsonValue>,
    error: Option<&str>,
    duration_ms: u64,
) -> Result<(), sqlx::Error> {
    let status = if error.is_some() { "failed" } else { "completed" };
    sqlx::query(
        "UPDATE rootcx_system.agent_tool_calls
         SET output = $2, error = $3, status = $4, duration_ms = $5
         WHERE id = $1::uuid",
    )
    .bind(call_id)
    .bind(output)
    .bind(error)
    .bind(status)
    .bind(duration_ms as i32)
    .execute(pool)
    .await?;
    Ok(())
}

async fn persist_session(
    pctx: &PersistCtx,
    assistant_response: &str,
    tokens: Option<u64>,
) -> Result<(), sqlx::Error> {
    ensure_session(&pctx.pool, &pctx.session_id, &pctx.app_id, pctx.user_id).await?;
    persist_message(&pctx.pool, &pctx.session_id, "user", &pctx.user_message, None, false).await?;
    persist_message(&pctx.pool, &pctx.session_id, "assistant", assistant_response, tokens.map(|t| t as i32), false).await?;

    let new_messages = json!([
        {"role": "user", "content": pctx.user_message},
        {"role": "assistant", "content": assistant_response}
    ]);
    sqlx::query(
        "UPDATE rootcx_system.agent_sessions SET
            messages = agent_sessions.messages || $2,
            total_tokens = COALESCE(agent_sessions.total_tokens, 0) + $3,
            turn_count = COALESCE(agent_sessions.turn_count, 0) + 1,
            updated_at = now()
         WHERE id = $1::uuid",
    )
    .bind(&pctx.session_id)
    .bind(&new_messages)
    .bind(tokens.unwrap_or(0) as i64)
    .execute(&pctx.pool)
    .await?;
    Ok(())
}
