use std::collections::HashMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::error::ForgeError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub directory: String,
    pub summary_message_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub error: Option<Value>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Part {
    pub id: String,
    pub message_id: String,
    pub part_type: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_state: Option<Value>,
    pub tool_input: Option<Value>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageWithParts {
    pub info: Message,
    pub parts: Vec<Part>,
}

fn now() -> String { Utc::now().to_rfc3339() }
fn new_id() -> String { Uuid::new_v4().to_string() }

fn parse_json_opt(s: Option<String>) -> Option<Value> {
    s.and_then(|v| serde_json::from_str(&v).ok())
}

fn row_to_session(row: &sqlx::sqlite::SqliteRow) -> Session {
    Session {
        id: row.get("id"),
        title: row.get("title"),
        directory: row.get("directory"),
        summary_message_id: row.get("summary_message_id"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn row_to_message(row: &sqlx::sqlite::SqliteRow) -> Message {
    Message {
        id: row.get("id"),
        session_id: row.get("session_id"),
        role: row.get("role"),
        error: parse_json_opt(row.get("error")),
        created_at: row.get("created_at"),
        completed_at: row.get("completed_at"),
    }
}

fn row_to_part(row: &sqlx::sqlite::SqliteRow) -> Part {
    Part {
        id: row.get("id"),
        message_id: row.get("message_id"),
        part_type: row.get("type"),
        content: row.get("content"),
        tool_name: row.get("tool_name"),
        tool_state: parse_json_opt(row.get("tool_state")),
        tool_input: parse_json_opt(row.get("tool_input")),
        created_at: row.get("created_at"),
    }
}

pub async fn create_session(pool: &SqlitePool, directory: &str) -> Result<Session, ForgeError> {
    let id = new_id();
    let ts = now();
    sqlx::query("INSERT INTO sessions (id, directory, created_at, updated_at) VALUES ($1, $2, $3, $4)")
        .bind(&id).bind(directory).bind(&ts).bind(&ts)
        .execute(pool).await?;
    Ok(Session { id, title: String::new(), directory: directory.into(), summary_message_id: None, created_at: ts.clone(), updated_at: ts })
}

pub async fn list_sessions(pool: &SqlitePool) -> Result<Vec<Session>, ForgeError> {
    let rows = sqlx::query("SELECT * FROM sessions ORDER BY updated_at DESC LIMIT 50")
        .fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_session).collect())
}

pub async fn insert_message(pool: &SqlitePool, session_id: &str, role: &str) -> Result<Message, ForgeError> {
    let id = new_id();
    let ts = now();
    sqlx::query("INSERT INTO messages (id, session_id, role, created_at) VALUES ($1, $2, $3, $4)")
        .bind(&id).bind(session_id).bind(role).bind(&ts)
        .execute(pool).await?;
    sqlx::query("UPDATE sessions SET updated_at = $1 WHERE id = $2")
        .bind(&ts).bind(session_id)
        .execute(pool).await?;
    Ok(Message { id, session_id: session_id.into(), role: role.into(), error: None, created_at: ts, completed_at: None })
}

pub async fn complete_message(pool: &SqlitePool, message_id: &str) -> Result<Message, ForgeError> {
    let ts = now();
    sqlx::query("UPDATE messages SET completed_at = $1 WHERE id = $2")
        .bind(&ts).bind(message_id)
        .execute(pool).await?;
    let row = sqlx::query("SELECT * FROM messages WHERE id = $1")
        .bind(message_id).fetch_one(pool).await?;
    Ok(row_to_message(&row))
}

pub async fn set_message_error(pool: &SqlitePool, message_id: &str, error: &Value) -> Result<(), ForgeError> {
    let ts = now();
    let err_str = serde_json::to_string(error).unwrap_or_default();
    sqlx::query("UPDATE messages SET error = $1, completed_at = $2 WHERE id = $3")
        .bind(&err_str).bind(&ts).bind(message_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn upsert_part(
    pool: &SqlitePool,
    id: &str,
    message_id: &str,
    part_type: &str,
    content: &str,
    tool_name: Option<&str>,
    tool_state: Option<&Value>,
    tool_input: Option<&Value>,
) -> Result<Part, ForgeError> {
    let ts = now();
    let state_str = tool_state.map(|v| serde_json::to_string(v).unwrap_or_default());
    let input_str = tool_input.map(|v| serde_json::to_string(v).unwrap_or_default());
    sqlx::query(
        "INSERT INTO parts (id, message_id, type, content, tool_name, tool_state, tool_input, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (id) DO UPDATE SET content = $4, tool_state = $6, tool_input = COALESCE(parts.tool_input, $7)"
    )
    .bind(id).bind(message_id).bind(part_type).bind(content)
    .bind(tool_name).bind(&state_str).bind(&input_str).bind(&ts)
    .execute(pool).await?;
    Ok(Part {
        id: id.into(), message_id: message_id.into(), part_type: part_type.into(),
        content: content.into(), tool_name: tool_name.map(Into::into),
        tool_state: tool_state.cloned(), tool_input: tool_input.cloned(),
        created_at: ts,
    })
}

pub async fn get_messages_with_parts(pool: &SqlitePool, session_id: &str) -> Result<Vec<MessageWithParts>, ForgeError> {
    let rows = sqlx::query("SELECT * FROM messages WHERE session_id = $1 ORDER BY created_at")
        .bind(session_id).fetch_all(pool).await?;
    let messages: Vec<Message> = rows.iter().map(row_to_message).collect();
    join_parts(pool, messages).await
}

pub async fn get_session(pool: &SqlitePool, session_id: &str) -> Result<Session, ForgeError> {
    let row = sqlx::query("SELECT * FROM sessions WHERE id = $1")
        .bind(session_id).fetch_one(pool).await?;
    Ok(row_to_session(&row))
}

pub async fn update_title(pool: &SqlitePool, session_id: &str, title: &str) -> Result<(), ForgeError> {
    sqlx::query("UPDATE sessions SET title = $1 WHERE id = $2")
        .bind(title).bind(session_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn set_summary_message_id(pool: &SqlitePool, session_id: &str, message_id: &str) -> Result<(), ForgeError> {
    sqlx::query("UPDATE sessions SET summary_message_id = $1 WHERE id = $2")
        .bind(message_id).bind(session_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn get_parts_for_message(pool: &SqlitePool, message_id: &str) -> Result<Vec<Part>, ForgeError> {
    let rows = sqlx::query("SELECT * FROM parts WHERE message_id = $1 ORDER BY created_at")
        .bind(message_id).fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_part).collect())
}

pub async fn get_messages_after(pool: &SqlitePool, session_id: &str, after_message_id: &str) -> Result<Vec<MessageWithParts>, ForgeError> {
    let boundary_row = sqlx::query("SELECT * FROM messages WHERE id = $1")
        .bind(after_message_id).fetch_one(pool).await?;
    let boundary = row_to_message(&boundary_row);
    let rows = sqlx::query("SELECT * FROM messages WHERE session_id = $1 AND created_at > $2 ORDER BY created_at")
        .bind(session_id).bind(&boundary.created_at)
        .fetch_all(pool).await?;
    let messages: Vec<Message> = rows.iter().map(row_to_message).collect();
    join_parts(pool, messages).await
}

async fn join_parts(pool: &SqlitePool, messages: Vec<Message>) -> Result<Vec<MessageWithParts>, ForgeError> {
    if messages.is_empty() {
        return Ok(vec![]);
    }
    // SQLite has no ANY($1) — build IN clause
    let placeholders: Vec<String> = (1..=messages.len()).map(|i| format!("${i}")).collect();
    let sql = format!(
        "SELECT * FROM parts WHERE message_id IN ({}) ORDER BY created_at",
        placeholders.join(", ")
    );
    let mut query = sqlx::query(&sql);
    for m in &messages {
        query = query.bind(&m.id);
    }
    let rows = query.fetch_all(pool).await?;
    let parts: Vec<Part> = rows.iter().map(row_to_part).collect();
    let mut grouped: HashMap<String, Vec<Part>> = HashMap::new();
    for p in parts {
        grouped.entry(p.message_id.clone()).or_default().push(p);
    }
    Ok(messages.into_iter().map(|msg| {
        let parts = grouped.remove(&msg.id).unwrap_or_default();
        MessageWithParts { info: msg, parts }
    }).collect())
}
