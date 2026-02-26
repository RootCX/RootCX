use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ForgeError;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub id: Uuid,
    pub title: String,
    pub directory: String,
    pub summary_message_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Message {
    pub id: Uuid,
    pub session_id: Uuid,
    pub role: String,
    pub error: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Part {
    pub id: Uuid,
    pub message_id: Uuid,
    #[sqlx(rename = "type")]
    pub part_type: String,
    pub content: String,
    pub tool_name: Option<String>,
    pub tool_state: Option<Value>,
    pub tool_input: Option<Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageWithParts {
    pub info: Message,
    pub parts: Vec<Part>,
}

// --- Queries ---

pub async fn create_session(pool: &PgPool, directory: &str) -> Result<Session, ForgeError> {
    Ok(sqlx::query_as(
        "INSERT INTO forge.sessions (directory) VALUES ($1) RETURNING *",
    )
    .bind(directory)
    .fetch_one(pool)
    .await?)
}

pub async fn list_sessions(pool: &PgPool) -> Result<Vec<Session>, ForgeError> {
    Ok(
        sqlx::query_as("SELECT * FROM forge.sessions ORDER BY updated_at DESC LIMIT 50")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn insert_message(
    pool: &PgPool,
    session_id: Uuid,
    role: &str,
) -> Result<Message, ForgeError> {
    let msg: Message = sqlx::query_as(
        "INSERT INTO forge.messages (session_id, role) VALUES ($1, $2) RETURNING *",
    )
    .bind(session_id)
    .bind(role)
    .fetch_one(pool)
    .await?;

    sqlx::query("UPDATE forge.sessions SET updated_at = now() WHERE id = $1")
        .bind(session_id)
        .execute(pool)
        .await?;

    Ok(msg)
}

pub async fn complete_message(pool: &PgPool, message_id: Uuid) -> Result<Message, ForgeError> {
    Ok(sqlx::query_as(
        "UPDATE forge.messages SET completed_at = now() WHERE id = $1 RETURNING *",
    )
    .bind(message_id)
    .fetch_one(pool)
    .await?)
}

pub async fn set_message_error(
    pool: &PgPool,
    message_id: Uuid,
    error: &Value,
) -> Result<(), ForgeError> {
    sqlx::query("UPDATE forge.messages SET error = $1, completed_at = now() WHERE id = $2")
        .bind(error)
        .bind(message_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn upsert_part(
    pool: &PgPool,
    id: Uuid,
    message_id: Uuid,
    part_type: &str,
    content: &str,
    tool_name: Option<&str>,
    tool_state: Option<&Value>,
    tool_input: Option<&Value>,
) -> Result<Part, ForgeError> {
    Ok(sqlx::query_as(
        r#"INSERT INTO forge.parts (id, message_id, type, content, tool_name, tool_state, tool_input)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (id) DO UPDATE SET content = $4, tool_state = $6, tool_input = COALESCE(forge.parts.tool_input, $7)
           RETURNING *"#,
    )
    .bind(id)
    .bind(message_id)
    .bind(part_type)
    .bind(content)
    .bind(tool_name)
    .bind(tool_state)
    .bind(tool_input)
    .fetch_one(pool)
    .await?)
}

pub async fn get_messages_with_parts(
    pool: &PgPool,
    session_id: Uuid,
) -> Result<Vec<MessageWithParts>, ForgeError> {
    let messages: Vec<Message> = sqlx::query_as(
        "SELECT * FROM forge.messages WHERE session_id = $1 ORDER BY created_at",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    join_parts(pool, messages).await
}

pub async fn get_session(pool: &PgPool, session_id: Uuid) -> Result<Session, ForgeError> {
    Ok(sqlx::query_as("SELECT * FROM forge.sessions WHERE id = $1")
        .bind(session_id)
        .fetch_one(pool)
        .await?)
}

pub async fn set_summary_message_id(
    pool: &PgPool,
    session_id: Uuid,
    message_id: Uuid,
) -> Result<(), ForgeError> {
    sqlx::query("UPDATE forge.sessions SET summary_message_id = $1 WHERE id = $2")
        .bind(message_id)
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_parts_for_message(pool: &PgPool, message_id: Uuid) -> Result<Vec<Part>, ForgeError> {
    Ok(sqlx::query_as("SELECT * FROM forge.parts WHERE message_id = $1 ORDER BY created_at")
        .bind(message_id)
        .fetch_all(pool)
        .await?)
}

pub async fn get_messages_after(
    pool: &PgPool,
    session_id: Uuid,
    after_message_id: Uuid,
) -> Result<Vec<MessageWithParts>, ForgeError> {
    let boundary: Message = sqlx::query_as("SELECT * FROM forge.messages WHERE id = $1")
        .bind(after_message_id)
        .fetch_one(pool)
        .await?;
    let messages: Vec<Message> = sqlx::query_as(
        "SELECT * FROM forge.messages WHERE session_id = $1 AND created_at > $2 ORDER BY created_at",
    )
    .bind(session_id)
    .bind(boundary.created_at)
    .fetch_all(pool)
    .await?;
    join_parts(pool, messages).await
}

async fn join_parts(pool: &PgPool, messages: Vec<Message>) -> Result<Vec<MessageWithParts>, ForgeError> {
    let ids: Vec<Uuid> = messages.iter().map(|m| m.id).collect();
    let parts: Vec<Part> = if ids.is_empty() {
        vec![]
    } else {
        sqlx::query_as("SELECT * FROM forge.parts WHERE message_id = ANY($1) ORDER BY created_at")
            .bind(&ids)
            .fetch_all(pool)
            .await?
    };
    let mut grouped: HashMap<Uuid, Vec<Part>> = HashMap::new();
    for p in parts {
        grouped.entry(p.message_id).or_default().push(p);
    }
    Ok(messages.into_iter().map(|msg| {
        let parts = grouped.remove(&msg.id).unwrap_or_default();
        MessageWithParts { info: msg, parts }
    }).collect())
}
