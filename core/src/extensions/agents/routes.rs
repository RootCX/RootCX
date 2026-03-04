use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};

use super::agent_user_id;
use super::approvals::{ApprovalReply, ApprovalResponse};
use super::config;
use super::persistence::PersistCtx;
use super::streaming;
use crate::api_error::ApiError;
use crate::auth::jwt;
use crate::auth::identity::Identity;
use crate::ipc::{AgentInvokePayload, RpcCaller};
use crate::routes::{self, SharedRuntime};

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

pub async fn get_agent(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<AgentRow>, ApiError> {
    let pool = routes::pool(&rt).await?;
    sqlx::query_as::<_, AgentRow>(
        "SELECT app_id, name, description, config FROM rootcx_system.agents WHERE app_id = $1",
    )
    .bind(&app_id).fetch_optional(&pool).await?
    .map(Json)
    .ok_or_else(|| ApiError::NotFound(format!("no agent for app '{app_id}'")))
}

pub async fn invoke_agent(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<InvokeRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (pool, wm, data_dir, auth_cfg, tool_registry) = {
        let g = rt.lock().await;
        (
            g.pool().cloned().ok_or(ApiError::NotReady)?,
            g.worker_manager().cloned().ok_or(ApiError::NotReady)?,
            g.data_dir().to_path_buf(),
            g.auth_config().clone(),
            g.tool_registry().clone(),
        )
    };

    let agent_config_json: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
    )
    .bind(&app_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound(format!("no agent for app '{app_id}'")))?;

    let session_id = body.session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let memory_enabled = agent_config_json.get("memory")
        .and_then(|m| m.get("enabled")).and_then(|e| e.as_bool()) == Some(true);

    let history = config::load_history(&pool, memory_enabled, &app_id, &session_id).await?;
    let system_prompt = config::load_system_prompt(&app_id, &agent_config_json, &data_dir).await?;
    let agent_config = config::build_agent_config(&pool, &app_id, &agent_config_json, &tool_registry).await?;

    let agent_token = jwt::encode_access(&auth_cfg, agent_user_id(&app_id), &format!("agent:{app_id}"))
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let persist_ctx = if memory_enabled {
        Some(PersistCtx {
            pool: pool.clone(), app_id: app_id.clone(),
            session_id: session_id.clone(), user_id: identity.user_id,
            user_message: body.message.clone(),
        })
    } else { None };

    let payload = AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        message: body.message,
        system_prompt,
        config: agent_config,
        auth_token: agent_token.clone(),
        history,
        caller: Some(RpcCaller { user_id: identity.user_id.to_string(), username: identity.username.clone(), auth_token: Some(agent_token) }),
    };

    let stream_rx = wm.agent_invoke(&app_id, payload).await?;

    Ok(Sse::new(streaming::build_sse_stream(stream_rx, session_id.into(), persist_ctx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

pub async fn list_sessions(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows = sqlx::query_as::<_, SessionRow>(
        "SELECT id::text, messages, created_at::text, updated_at::text,
                title, status, total_tokens, turn_count
         FROM rootcx_system.agent_sessions WHERE app_id = $1 ORDER BY updated_at DESC",
    ).bind(&app_id).fetch_all(&pool).await?;
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
         FROM rootcx_system.agent_sessions WHERE app_id = $1 AND id = $2::uuid",
    ).bind(&app_id).bind(&session_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound(format!("session '{session_id}' not found")))?;

    let messages = sqlx::query_as::<_, MessageRow>(
        "SELECT id::text, role, content, token_count, is_summary, created_at::text
         FROM rootcx_system.agent_messages WHERE session_id = $1::uuid ORDER BY created_at ASC",
    ).bind(&session_id).fetch_all(&pool).await?;

    if messages.is_empty() {
        return Ok(Json(json!({
            "id": session.id, "title": session.title, "status": session.status,
            "totalTokens": session.total_tokens, "turnCount": session.turn_count,
            "messages": session.messages, "createdAt": session.created_at, "updatedAt": session.updated_at,
        })));
    }

    let tool_calls = sqlx::query_as::<_, ToolCallRow>(
        "SELECT id::text, tool_name, input, output, error, status, duration_ms, created_at::text
         FROM rootcx_system.agent_tool_calls WHERE session_id = $1::uuid ORDER BY created_at ASC",
    ).bind(&session_id).fetch_all(&pool).await?;

    let structured: Vec<JsonValue> = messages.iter().enumerate().map(|(i, m)| {
        let mut msg = json!({
            "id": m.id, "role": m.role, "content": m.content,
            "tokenCount": m.token_count, "isSummary": m.is_summary, "createdAt": m.created_at,
        });
        if m.role == "assistant" {
            let upper = messages.get(i + 1).map(|next| next.created_at.as_str());
            let tc: Vec<&ToolCallRow> = tool_calls.iter().filter(|tc| {
                tc.created_at >= m.created_at && upper.is_none_or(|u| tc.created_at.as_str() < u)
            }).collect();
            if !tc.is_empty() { msg["toolCalls"] = json!(tc); }
        }
        msg
    }).collect();

    Ok(Json(json!({
        "id": session.id, "title": session.title, "status": session.status,
        "totalTokens": session.total_tokens, "turnCount": session.turn_count,
        "messages": structured, "createdAt": session.created_at, "updatedAt": session.updated_at,
    })))
}

pub async fn get_session_events(
    State(rt): State<SharedRuntime>,
    Path((app_id, session_id)): Path<(String, String)>,
) -> Result<Json<Vec<SessionEventEntry>>, ApiError> {
    let pool = routes::pool(&rt).await?;

    // Verify session belongs to app
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.agent_sessions WHERE id = $1::uuid AND app_id = $2)",
    ).bind(&session_id).bind(&app_id).fetch_one(&pool).await?;
    if !exists { return Err(ApiError::NotFound(format!("session '{session_id}' not found"))); }

    let messages = sqlx::query_as::<_, MessageRow>(
        "SELECT id::text, role, content, token_count, is_summary, created_at::text
         FROM rootcx_system.agent_messages WHERE session_id = $1::uuid ORDER BY created_at ASC",
    ).bind(&session_id).fetch_all(&pool).await?;

    let tool_calls = sqlx::query_as::<_, ToolCallRow>(
        "SELECT id::text, tool_name, input, output, error, status, duration_ms, created_at::text
         FROM rootcx_system.agent_tool_calls WHERE session_id = $1::uuid ORDER BY created_at ASC",
    ).bind(&session_id).fetch_all(&pool).await?;

    let mut events: Vec<SessionEventEntry> = Vec::new();
    for m in &messages {
        events.push(SessionEventEntry {
            event_type: "message".into(),
            data: json!({"id": m.id, "role": m.role, "content": m.content, "tokenCount": m.token_count, "isSummary": m.is_summary, "at": m.created_at}),
        });
    }
    for tc in &tool_calls {
        events.push(SessionEventEntry {
            event_type: "tool_call".into(),
            data: json!({"id": tc.id, "toolName": tc.tool_name, "input": tc.input, "output": tc.output, "error": tc.error, "status": tc.status, "durationMs": tc.duration_ms, "at": tc.created_at}),
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
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<super::approvals::ApprovalRequest>>, ApiError> {
    let approvals = rt.lock().await.pending_approvals().clone();
    Ok(Json(approvals.list(&app_id).await))
}

pub async fn reply_approval(
    State(rt): State<SharedRuntime>,
    Path((app_id, approval_id)): Path<(String, String)>,
    Json(body): Json<ApprovalReply>,
) -> Result<Json<JsonValue>, ApiError> {
    let approvals = rt.lock().await.pending_approvals().clone();
    // Verify approval belongs to app
    if !approvals.list(&app_id).await.iter().any(|a| a.approval_id == approval_id) {
        return Err(ApiError::NotFound(format!("approval '{approval_id}' not found for app '{app_id}'")));
    }
    let response = match body.action {
        super::approvals::ApprovalAction::Approve => ApprovalResponse::Approved,
        super::approvals::ApprovalAction::Reject => ApprovalResponse::Rejected {
            reason: body.reason.unwrap_or_else(|| "rejected by user".into()),
        },
    };
    if approvals.reply(&approval_id, response).await {
        Ok(Json(json!({"status": "ok"})))
    } else {
        Err(ApiError::NotFound(format!("approval '{approval_id}' not found")))
    }
}
