use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;

const QUEUE: &str = "jobs";
const DLQ: &str = "jobs_dlq";
pub const VISIBILITY_TIMEOUT_SECS: i32 = 120;
pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
pub const MAX_DELIVERIES: i32 = 5;

pub fn delivery_exhausted(read_count: i32) -> bool {
    read_count > MAX_DELIVERIES
}

fn err(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Job(e.to_string())
}

pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgmq").execute(pool).await.map_err(err)?;
    sqlx::query(&format!("SELECT pgmq.create('{QUEUE}')"))
        .execute(pool).await.map_err(err)?;
    sqlx::query(&format!("SELECT pgmq.create('{DLQ}')"))
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
    /// Core-owned cron provenance. It is outside `payload`, so app callers of
    /// `enqueueJob` and the HTTP jobs API cannot manufacture workflow authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron_id: Option<uuid::Uuid>,
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
        app_id: app_id.to_string(), payload, user_id, cron_id: None,
    }).map_err(err)?;
    let (msg_id,): (i64,) = sqlx::query_as(&format!("SELECT pgmq.send('{QUEUE}', $1)"))
        .bind(&msg).fetch_one(pool).await.map_err(err)?;
    info!(msg_id, app_id, "job enqueued");
    Ok(msg_id)
}

pub async fn enqueue_cron(
    pool: &PgPool,
    app_id: &str,
    cron_id: uuid::Uuid,
    payload: JsonValue,
    user_id: Option<uuid::Uuid>,
) -> Result<i64, RuntimeError> {
    let msg = serde_json::to_value(JobMessage {
        app_id: app_id.to_string(),
        payload,
        user_id,
        cron_id: Some(cron_id),
    }).map_err(err)?;
    let (msg_id,): (i64,) = sqlx::query_as(&format!("SELECT pgmq.send('{QUEUE}', $1)"))
        .bind(&msg).fetch_one(pool).await.map_err(err)?;
    info!(msg_id, app_id, %cron_id, "cron job enqueued");
    Ok(msg_id)
}

/// Returns `(msg_id, read_ct, message)`. `read_ct` counts deliveries: > 1 means
/// the previous lease expired (worker crash or overrun) and this is a redelivery.
pub async fn read_next(pool: &PgPool) -> Result<Option<(i64, i32, JobMessage)>, RuntimeError> {
    let row: Option<(i64, i32, JsonValue)> = sqlx::query_as(
        &format!("SELECT msg_id, read_ct, message FROM pgmq.read('{QUEUE}', {VISIBILITY_TIMEOUT_SECS}, 1)")
    ).fetch_optional(pool).await.map_err(err)?;

    match row {
        Some((msg_id, read_ct, message)) => {
            let job_msg: JobMessage = serde_json::from_value(message).map_err(err)?;
            Ok(Some((msg_id, read_ct, job_msg)))
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

/// Extend the visibility timeout of an in-flight message (lease heartbeat): keeps
/// a long-running job from being redelivered while a worker is still on it.
pub async fn extend_lease(pool: &PgPool, msg_id: i64, vt_secs: i32) -> Result<(), RuntimeError> {
    sqlx::query(&format!("SELECT pgmq.set_vt('{QUEUE}', $1, {vt_secs})"))
        .bind(msg_id).execute(pool).await.map_err(err)?;
    Ok(())
}

/// Move a poison message off the main queue: copy it to the dead-letter queue
/// (with the failure reason) and archive the original. Terminal — never retried.
pub async fn dead_letter(pool: &PgPool, msg_id: i64, message: &JsonValue, reason: &str) -> Result<(), RuntimeError> {
    let mut entry = message.clone();
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("_dlq_reason".into(), JsonValue::String(reason.to_string()));
        obj.insert("_dlq_msg_id".into(), JsonValue::Number(msg_id.into()));
    }
    sqlx::query(&format!("SELECT pgmq.send('{DLQ}', $1)"))
        .bind(&entry).execute(pool).await.map_err(err)?;
    sqlx::query(&format!("SELECT pgmq.archive('{QUEUE}', $1)"))
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

#[cfg(test)]
mod tests {
    use super::{delivery_exhausted, JobMessage, MAX_DELIVERIES};
    use serde_json::json;

    #[test]
    fn delivery_limit_allows_last_attempt_then_exhausts() {
        assert!(!delivery_exhausted(MAX_DELIVERIES));
        assert!(delivery_exhausted(MAX_DELIVERIES + 1));
    }

    #[test]
    fn payload_cannot_forge_cron_provenance() {
        let forged = uuid::Uuid::new_v4();
        let message = serde_json::to_value(JobMessage {
            app_id: "app".into(),
            payload: json!({"cron_id": forged, "type": "forged"}),
            user_id: None,
            cron_id: None,
        }).unwrap();

        assert!(message.get("cron_id").is_none());
        assert_eq!(message["payload"]["cron_id"], forged.to_string());
    }
}
