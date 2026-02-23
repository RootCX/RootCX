use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::ipc::RpcCaller;
use crate::routes::{self, SharedRuntime};
use crate::worker::AgentEvent;

#[derive(Deserialize)]
pub struct InvokeRequest {
    pub message: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct AgentRow {
    id: String,
    app_id: String,
    name: String,
    description: Option<String>,
    model: Option<String>,
    config: JsonValue,
}

#[derive(Serialize)]
pub(crate) struct SessionRow {
    id: String,
    agent_id: String,
    messages: JsonValue,
    created_at: String,
    updated_at: String,
}

pub async fn list_agents(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<AgentRow>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>, JsonValue)>(
        "SELECT id, app_id, name, description, model, config
         FROM rootcx_system.agents WHERE app_id = $1 ORDER BY name",
    )
    .bind(&app_id)
    .fetch_all(&pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, app_id, name, description, model, config)| AgentRow {
                id, app_id, name, description, model, config,
            })
            .collect(),
    ))
}

pub async fn invoke_agent(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, agent_id)): Path<(String, String)>,
    Json(body): Json<InvokeRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (pool, wm, data_dir) = {
        let g = rt.lock().await;
        let pool = g.pool().cloned().ok_or(ApiError::NotReady)?;
        let wm = g.worker_manager().cloned().ok_or(ApiError::NotReady)?;
        let data_dir = g.data_dir().to_path_buf();
        (pool, wm, data_dir)
    };

    let config: JsonValue = sqlx::query_scalar(
        "SELECT config FROM rootcx_system.agents WHERE app_id = $1 AND id = $2",
    )
    .bind(&app_id)
    .bind(&agent_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("agent '{agent_id}' not found")))?;
    let session_id = body
        .session_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let memory_enabled = config
        .get("memory")
        .and_then(|m| m.get("enabled"))
        .and_then(|e| e.as_bool())
        .unwrap_or(false);

    let history: Vec<JsonValue> = if memory_enabled {
        sqlx::query_scalar::<_, JsonValue>(
            "SELECT messages FROM rootcx_system.agent_sessions WHERE id = $1::uuid",
        )
        .bind(&session_id)
        .fetch_optional(&pool)
        .await?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
    } else {
        vec![]
    };

    let system_prompt = load_system_prompt(&app_id, &agent_id, &config, &data_dir).await?;
    let enabled_tools = extract_enabled_tools(&config);
    let agent_config = json!({
        "model": config.get("model"),
        "limits": config.get("limits"),
        "_appId": &app_id,
        "_enabledTools": enabled_tools,
        "_graphAbsolutePath": config.get("graph"),
    });

    let caller = Some(RpcCaller {
        user_id: identity.user_id.to_string(),
        username: identity.username.clone(),
    });

    let stream_rx = wm
        .agent_invoke(
            &app_id,
            agent_id.clone(),
            session_id.clone(),
            body.message.clone(),
            system_prompt,
            agent_config,
            history,
            caller,
        )
        .await?;

    let pool_for_stream = pool.clone();
    let sid = session_id.clone();
    let aid = app_id.clone();
    let agid = agent_id.clone();
    let uid = Some(identity.user_id.to_string());
    let user_msg = body.message;

    let stream = futures::stream::unfold(
        (stream_rx, false),
        move |(mut rx, done)| {
            let pool = pool_for_stream.clone();
            let sid = sid.clone();
            let aid = aid.clone();
            let agid = agid.clone();
            let uid = uid.clone();
            let umsg = user_msg.clone();
            async move {
                if done {
                    return None;
                }
                match rx.recv().await {
                    Some(AgentEvent::Chunk { delta }) => {
                        let event = Event::default()
                            .event("chunk")
                            .data(json!({"delta": delta, "session_id": sid}).to_string());
                        Some((Ok(event), (rx, false)))
                    }
                    Some(AgentEvent::Done { response, tokens }) => {
                        let _ = persist_session(&pool, &sid, &aid, &agid, uid.as_deref(), &umsg, &response).await;
                        let event = Event::default()
                            .event("done")
                            .data(
                                json!({"response": response, "session_id": sid, "tokens": tokens})
                                    .to_string(),
                            );
                        Some((Ok(event), (rx, true)))
                    }
                    Some(AgentEvent::Error { error }) => {
                        let event = Event::default()
                            .event("error")
                            .data(json!({"error": error, "session_id": sid}).to_string());
                        Some((Ok(event), (rx, true)))
                    }
                    None => None,
                }
            }
        },
    );

    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

pub async fn list_sessions(
    State(rt): State<SharedRuntime>,
    Path((app_id, agent_id)): Path<(String, String)>,
) -> Result<Json<Vec<SessionRow>>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let rows = sqlx::query_as::<_, (String, String, JsonValue, String, String)>(
        "SELECT id::text, agent_id, messages, created_at::text, updated_at::text
         FROM rootcx_system.agent_sessions
         WHERE app_id = $1 AND agent_id = $2
         ORDER BY updated_at DESC",
    )
    .bind(&app_id)
    .bind(&agent_id)
    .fetch_all(&pool)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|(id, agent_id, messages, created_at, updated_at)| SessionRow {
                id, agent_id, messages, created_at, updated_at,
            })
            .collect(),
    ))
}

pub async fn get_session(
    State(rt): State<SharedRuntime>,
    Path((app_id, agent_id, session_id)): Path<(String, String, String)>,
) -> Result<Json<SessionRow>, ApiError> {
    let pool = routes::pool(&rt).await?;
    let row = sqlx::query_as::<_, (String, String, JsonValue, String, String)>(
        "SELECT id::text, agent_id, messages, created_at::text, updated_at::text
         FROM rootcx_system.agent_sessions
         WHERE app_id = $1 AND agent_id = $2 AND id = $3::uuid",
    )
    .bind(&app_id)
    .bind(&agent_id)
    .bind(&session_id)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("session '{session_id}' not found")))?;

    Ok(Json(SessionRow {
        id: row.0,
        agent_id: row.1,
        messages: row.2,
        created_at: row.3,
        updated_at: row.4,
    }))
}

fn extract_enabled_tools(config: &JsonValue) -> Vec<String> {
    let mut tools = vec![];
    let access = config.get("access").and_then(|a| a.as_array());
    let mut has_data_access = false;
    if let Some(entries) = access {
        for entry in entries {
            let entity = entry
                .get("entity")
                .and_then(|e| e.as_str())
                .unwrap_or("");
            if let Some(tool_name) = entity.strip_prefix("tool:") {
                tools.push(tool_name.to_string());
            } else if !entity.is_empty() {
                has_data_access = true;
            }
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
    agent_id: &str,
    config: &JsonValue,
    data_dir: &std::path::Path,
) -> Result<String, ApiError> {
    let default_path = format!("./agents/{agent_id}/system.md");
    let prompt_path = config
        .get("systemPrompt")
        .and_then(|p| p.as_str())
        .unwrap_or(&default_path);
    let app_dir = data_dir.join("apps").join(app_id);
    let full_path = app_dir.join(prompt_path.trim_start_matches("./"));
    tokio::fs::read_to_string(&full_path).await.map_err(|e| {
        ApiError::Internal(format!(
            "failed to read system prompt at {}: {e}",
            full_path.display()
        ))
    })
}

async fn persist_session(
    pool: &PgPool,
    session_id: &str,
    app_id: &str,
    agent_id: &str,
    user_id: Option<&str>,
    user_message: &str,
    assistant_response: &str,
) -> Result<(), sqlx::Error> {
    let new_messages = json!([
        {"role": "user", "content": user_message},
        {"role": "assistant", "content": assistant_response}
    ]);
    sqlx::query(
        "INSERT INTO rootcx_system.agent_sessions (id, app_id, agent_id, user_id, messages)
         VALUES ($1::uuid, $2, $3, $4, $5)
         ON CONFLICT (id) DO UPDATE SET
             messages = rootcx_system.agent_sessions.messages || $5,
             updated_at = now()",
    )
    .bind(session_id)
    .bind(app_id)
    .bind(agent_id)
    .bind(user_id)
    .bind(&new_messages)
    .execute(pool)
    .await?;
    Ok(())
}
