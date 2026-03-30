use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;

const QUEUE: &str = "jobs";
const VT_SECS: i32 = 120;

fn err(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Job(e.to_string())
}

pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgmq").execute(pool).await.map_err(err)?;
    sqlx::query(&format!("SELECT pgmq.create('{QUEUE}')"))
        .execute(pool).await.map_err(err)?;
    info!("pgmq jobs queue ready");
    Ok(())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct JobMessage {
    pub app_id: String,
    pub payload: JsonValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<uuid::Uuid>,
}

#[derive(Debug, serde::Serialize)]
pub struct Job {
    pub msg_id: i64,
    pub app_id: String,
    pub payload: JsonValue,
    pub user_id: Option<uuid::Uuid>,
    pub read_ct: i32,
    pub enqueued_at: String,
}

pub async fn enqueue(pool: &PgPool, app_id: &str, payload: JsonValue, user_id: Option<uuid::Uuid>) -> Result<i64, RuntimeError> {
    let msg = serde_json::to_value(JobMessage {
        app_id: app_id.to_string(), payload, user_id,
    }).map_err(err)?;
    let (msg_id,): (i64,) = sqlx::query_as(&format!("SELECT pgmq.send('{QUEUE}', $1)"))
        .bind(&msg).fetch_one(pool).await.map_err(err)?;
    info!(msg_id, app_id, "job enqueued");
    Ok(msg_id)
}

pub async fn read_next(pool: &PgPool) -> Result<Option<(i64, JobMessage)>, RuntimeError> {
    let row: Option<(i64, JsonValue)> = sqlx::query_as(
        &format!("SELECT msg_id, message FROM pgmq.read('{QUEUE}', {VT_SECS}, 1)")
    ).fetch_optional(pool).await.map_err(err)?;

    match row {
        Some((msg_id, message)) => {
            let job_msg: JobMessage = serde_json::from_value(message).map_err(err)?;
            Ok(Some((msg_id, job_msg)))
        }
        None => Ok(None),
    }
}

pub async fn complete(pool: &PgPool, msg_id: i64) -> Result<(), RuntimeError> {
    sqlx::query(&format!("SELECT pgmq.archive('{QUEUE}', $1)"))
        .bind(msg_id).execute(pool).await.map_err(err)?;
    Ok(())
}

pub async fn fail(pool: &PgPool, msg_id: i64) -> Result<(), RuntimeError> {
    sqlx::query(&format!("SELECT pgmq.delete('{QUEUE}', $1)"))
        .bind(msg_id).execute(pool).await.map_err(err)?;
    Ok(())
}

async fn list_from(pool: &PgPool, table: &str, ts_col: &str, app_id: &str, limit: i64) -> Result<Vec<Job>, RuntimeError> {
    let sql = format!(
        "SELECT msg_id, read_ct, {ts_col}::text, message FROM pgmq.{table} WHERE message->>'app_id' = $1 ORDER BY msg_id DESC LIMIT $2"
    );
    let rows: Vec<(i64, i32, String, JsonValue)> = sqlx::query_as(&sql)
        .bind(app_id).bind(limit).fetch_all(pool).await.map_err(err)?;

    Ok(rows.into_iter().filter_map(|(msg_id, read_ct, enqueued_at, message)| {
        let m: JobMessage = serde_json::from_value(message).ok()?;
        Some(Job { msg_id, app_id: m.app_id, payload: m.payload, user_id: m.user_id, read_ct, enqueued_at })
    }).collect())
}

pub async fn list_for_app(pool: &PgPool, app_id: &str, limit: i64) -> Result<Vec<Job>, RuntimeError> {
    list_from(pool, &format!("q_{QUEUE}"), "enqueued_at", app_id, limit).await
}

pub async fn list_archived(pool: &PgPool, app_id: &str, limit: i64) -> Result<Vec<Job>, RuntimeError> {
    list_from(pool, &format!("a_{QUEUE}"), "archived_at", app_id, limit).await
}
