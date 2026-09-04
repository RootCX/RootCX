use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;

const QUEUE: &str = "jobs";
const DLQ: &str = "jobs_dlq";
pub const VISIBILITY_TIMEOUT_SECS: i32 = 120;
pub const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
pub const MAX_DELIVERIES: i32 = 5;

/// Ceiling on how long one delivery may hold its lease alive by heartbeat.
///
/// The heartbeat used to renew forever, so a handler that hung — a worker that
/// never answered, a node stuck on a request with no timeout — pinned its
/// message in `running` for the life of the process: never redelivered, never
/// dead-lettered, never visible. Renewal stops here so the ordinary
/// redelivery-then-DLQ path can do its job. Deliberately far past any real job:
/// crossing it is evidence of a hang, not of slow work.
pub const MAX_LEASE_LIFETIME: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

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

/// Keep one leased message alive while its handler runs, until the handler
/// finishes (`cancel`), the queue rejects a renewal, or `lifetime` elapses.
///
/// One implementation for both callers (app jobs and workflow runs): they had
/// the same loop twice, and only one of them was ever fixed at a time.
pub async fn lease_heartbeat(
    pool: PgPool,
    msg_id: i64,
    cancel: tokio_util::sync::CancellationToken,
    lifetime: std::time::Duration,
) {
    let deadline = tokio::time::Instant::now() + lifetime;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep_until(deadline) => {
                tracing::error!(msg_id, "lease held past the maximum lifetime; releasing it \
                    to redelivery (the handler is presumed hung)");
                return;
            }
            _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {
                // Transient renewal failures are retried: surrendering the lease
                // under a live handler would redeliver the message and start a
                // second concurrent run.
                if let Err(e) = extend_lease(&pool, msg_id, VISIBILITY_TIMEOUT_SECS).await {
                    tracing::warn!(msg_id, "lease heartbeat failed: {e}");
                }
            }
        }
    }
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

    // Renewal used to be unbounded, so a handler that hung pinned its message
    // forever: never redelivered, never dead-lettered, never visible. The
    // ceiling is what hands a hung unit of work back to the redelivery path.
    #[tokio::test(start_paused = true)]
    async fn lease_renewal_stops_at_the_ceiling() {
        // Unreachable on purpose: the heartbeat must survive failing renewals
        // (surrendering a live lease would start a second concurrent run), so
        // only the ceiling can end this loop.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/nonexistent").unwrap();
        let lifetime = std::time::Duration::from_secs(600);

        tokio::time::timeout(
            lifetime * 3,
            super::lease_heartbeat(pool, 1, tokio_util::sync::CancellationToken::new(), lifetime),
        ).await.expect("an unbounded heartbeat hides a hung handler forever");
    }

    // The ordinary path: a handler that finishes cancels its own heartbeat, and
    // must not be kept alive until the ceiling.
    #[tokio::test(start_paused = true)]
    async fn a_finished_handler_ends_its_heartbeat_at_once() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/nonexistent").unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        tokio::time::timeout(
            super::HEARTBEAT_INTERVAL,
            super::lease_heartbeat(pool, 1, cancel, super::MAX_LEASE_LIFETIME),
        ).await.expect("cancellation must end the heartbeat without waiting");
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
