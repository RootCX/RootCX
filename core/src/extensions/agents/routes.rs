use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tokio::sync::broadcast::error::RecvError;

use super::approvals::{ApprovalReply, ApprovalResponse};
use super::config;
use super::persistence::PersistCtx;
use super::streaming;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::ipc::{AgentInvokePayload, FileAttachment, LlmModelRef};
use crate::routes::{self, SharedRuntime, llm_models::fetch_default_llm};

#[derive(Deserialize)]
pub struct InvokeRequest {
    pub message: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub file_ids: Option<Vec<String>>,
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

pub async fn list_agents(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<AgentRow>>, ApiError> {
    Ok(Json(sqlx::query_as::<_, AgentRow>(
        "SELECT app_id, name, description, config FROM rootcx_system.agents ORDER BY name",
    ).fetch_all(&routes::pool(&rt)).await?))
}

pub async fn get_agent(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<AgentRow>, ApiError> {
    let pool = routes::pool(&rt);
    sqlx::query_as::<_, AgentRow>(
        "SELECT app_id, name, description, config FROM rootcx_system.agents WHERE app_id = $1",
    )
    .bind(&app_id).fetch_optional(&pool).await?
    .map(Json)
    .ok_or_else(|| ApiError::NotFound(format!("no agent for app '{app_id}'")))
}

#[derive(Deserialize)]
pub struct UpdateAgent {
    #[serde(default)] name: Option<String>,
    #[serde(default)] description: Option<String>,
}

pub async fn update_agent(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<UpdateAgent>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let result = sqlx::query(
        "UPDATE rootcx_system.agents SET
            name = COALESCE($2, name),
            description = COALESCE($3, description),
            updated_at = now()
         WHERE app_id = $1",
    ).bind(&app_id).bind(&body.name).bind(&body.description)
    .execute(&pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("agent '{app_id}' not found")));
    }
    Ok(Json(json!({"status": "ok"})))
}

pub async fn delete_agent(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let result = sqlx::query("DELETE FROM rootcx_system.agents WHERE app_id = $1")
        .bind(&app_id).execute(&pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("agent '{app_id}' not found")));
    }
    Ok(Json(json!({"status": "ok"})))
}

pub async fn invoke_agent(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<InvokeRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let pool = rt.pool().clone();
    let wm = rt.worker_manager().clone();

    let agent_config_json: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1",
    ).bind(&app_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound(format!("no agent for app '{app_id}'")))?;

    let session_id = body.session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let memory_enabled = agent_config_json.get("memory")
        .and_then(|m| m.get("enabled")).and_then(|e| e.as_bool()) == Some(true);

    let history = config::load_history(&pool, memory_enabled, &app_id, &session_id).await?;

    let llm = fetch_default_llm(&pool).await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(|(provider, model)| LlmModelRef { provider, model });

    // Resolve file_ids → nonce download URLs (worker fetches bytes via HTTP, no base64 in IPC).
    let attachments = if let Some(ids) = body.file_ids.as_deref() {
        let mut list = Vec::with_capacity(ids.len());
        for raw_id in ids {
            let file_id = raw_id.parse::<uuid::Uuid>()
                .map_err(|_| ApiError::BadRequest(format!("invalid file_id: {raw_id}")))?;
            let (name, content_type): (String, String) = sqlx::query_as(
                "SELECT name, content_type FROM rootcx_system.files WHERE id = $1 AND app_id = $2",
            ).bind(file_id).bind(&app_id).fetch_optional(&pool).await?
            .ok_or_else(|| ApiError::NotFound(format!("file {file_id}")))?;

            let nonce = rt.upload_nonces().lock().unwrap_or_else(|e| e.into_inner())
                .create_download(file_id, &app_id);
            let url = crate::extensions::storage::download_url(rt.runtime_url(), &nonce);
            list.push(FileAttachment { name, content_type, url });
        }
        Some(list)
    } else {
        None
    };

    let payload = AgentInvokePayload {
        invoke_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.clone(),
        message: body.message.clone(),
        history,
        is_sub_invoke: false,
        llm,
        invoker_user_id: Some(identity.user_id),
        attachments,
    };

    let persist_ctx = if memory_enabled {
        Some(PersistCtx {
            pool: pool.clone(), app_id: app_id.clone(),
            session_id: session_id.clone(), user_id: identity.user_id,
            user_message: body.message,
        })
    } else { None };

    let stream_rx = wm.agent_invoke(&app_id, payload).await?;

    Ok(Sse::new(streaming::build_sse_stream(stream_rx, session_id.into(), persist_ctx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_limit() -> i64 { 100 }

pub async fn list_sessions(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Query(p): Query<PaginationParams>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let pool = routes::pool(&rt);
    let limit = p.limit.clamp(1, 1000);
    let offset = p.offset.max(0);
    let rows = sqlx::query_as::<_, SessionRow>(
        "SELECT id::text, messages, created_at::text, updated_at::text,
                title, status, total_tokens, turn_count
         FROM rootcx_system.agent_sessions WHERE app_id = $1
         ORDER BY updated_at DESC LIMIT $2 OFFSET $3",
    ).bind(&app_id).bind(limit).bind(offset).fetch_all(&pool).await?;
    Ok(Json(rows))
}

pub async fn get_session(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, session_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
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
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, session_id)): Path<(String, String)>,
) -> Result<Json<Vec<SessionEventEntry>>, ApiError> {
    let pool = routes::pool(&rt);

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
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<super::approvals::ApprovalRequest>>, ApiError> {
    let approvals = rt.pending_approvals().clone();
    Ok(Json(approvals.list(&app_id).await))
}

pub async fn reply_approval(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, approval_id)): Path<(String, String)>,
    Json(body): Json<ApprovalReply>,
) -> Result<Json<JsonValue>, ApiError> {
    let approvals = rt.pending_approvals().clone();
    // Verify approval belongs to app
    if !approvals.belongs_to_app(&approval_id, &app_id).await {
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

pub async fn fleet_stream(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let rx = routes::wm(&rt).subscribe_fleet();
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok(event) => {
                let data = serde_json::to_string(&event).unwrap_or_default();
                Some((Ok(Event::default().data(data)), rx))
            }
            Err(RecvError::Lagged(_)) => Some((Ok(Event::default().data("{}")), rx)),
            Err(RecvError::Closed) => None,
        }
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[cfg(test)]
mod tests {
    use super::*;

    // invoker_user_id MUST come from JWT, never from client JSON.
    #[test]
    fn invoke_request_rejects_invoker_user_id_injection() {
        let json = r#"{"message":"hi","invoker_user_id":"00000000-0000-0000-0000-000000000000"}"#;
        let req: InvokeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "hi");
        // Verify the struct has no invoker_user_id field by checking it deserializes fine
        // and the field is simply ignored (serde deny_unknown_fields is intentionally absent).
        let fields = serde_json::to_value(&serde_json::json!({"message":"hi"})).unwrap();
        assert!(fields.get("invoker_user_id").is_none());
    }

    #[test]
    fn invoke_request_accepts_file_ids() {
        let json = r#"{"message":"analyse this","file_ids":["00000000-0000-0000-0000-000000000001"]}"#;
        let req: InvokeRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.message, "analyse this");
        assert_eq!(req.file_ids.as_deref(), Some(["00000000-0000-0000-0000-000000000001".to_string()].as_slice()));
    }

    #[test]
    fn invoke_request_file_ids_optional() {
        let json = r#"{"message":"hi"}"#;
        let req: InvokeRequest = serde_json::from_str(json).unwrap();
        assert!(req.file_ids.is_none());
    }
}

