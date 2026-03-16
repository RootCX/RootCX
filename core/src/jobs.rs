use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use crate::RuntimeError;

type JobRow = (Uuid, String, String, Option<JsonValue>, Option<JsonValue>, Option<String>, i32, Option<Uuid>);

const COLS: &str = "id, app_id, status, payload, result, error, attempts, user_id";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Job {
    pub id: Uuid,
    pub app_id: String,
    pub status: String,
    pub payload: Option<JsonValue>,
    pub result: Option<JsonValue>,
    pub error: Option<String>,
    pub attempts: i32,
    pub user_id: Option<Uuid>,
}

impl From<JobRow> for Job {
    fn from((id, app_id, status, payload, result, error, attempts, user_id): JobRow) -> Self {
        Self { id, app_id, status, payload, result, error, attempts, user_id }
    }
}

fn err(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Job(e.to_string())
}

pub async fn bootstrap_jobs_schema(pool: &PgPool) -> Result<(), RuntimeError> {
    for sql in [
        "CREATE TABLE IF NOT EXISTS rootcx_system.jobs (
            id UUID PRIMARY KEY, app_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending', payload JSONB, result JSONB, error TEXT,
            attempts INT NOT NULL DEFAULT 0, run_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            user_id UUID,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )",
        "ALTER TABLE rootcx_system.jobs ADD COLUMN IF NOT EXISTS user_id UUID",
        "CREATE INDEX IF NOT EXISTS idx_jobs_pending ON rootcx_system.jobs (run_at) WHERE status = 'pending'",
        "CREATE INDEX IF NOT EXISTS idx_jobs_app ON rootcx_system.jobs (app_id, status)",
    ] {
        sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    }
    info!("jobs schema ready");
    Ok(())
}

pub async fn enqueue(
    pool: &PgPool,
    app_id: &str,
    payload: JsonValue,
    run_at: Option<chrono::DateTime<chrono::Utc>>,
    user_id: Option<Uuid>,
) -> Result<Uuid, RuntimeError> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.jobs (id, app_id, payload, run_at, user_id) VALUES ($1, $2, $3, $4, $5)")
        .bind(id)
        .bind(app_id)
        .bind(&payload)
        .bind(run_at.unwrap_or_else(chrono::Utc::now))
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(err)?;
    info!(job_id = %id, app_id, "job enqueued");
    Ok(id)
}

pub async fn claim_next(pool: &PgPool) -> Result<Option<Job>, RuntimeError> {
    sqlx::query_as::<_, JobRow>(&format!(
        "UPDATE rootcx_system.jobs SET status = 'running', attempts = attempts + 1, updated_at = now()
         WHERE id = (
             SELECT id FROM rootcx_system.jobs
             WHERE status = 'pending' AND run_at <= now()
             ORDER BY run_at FOR UPDATE SKIP LOCKED LIMIT 1
         ) RETURNING {COLS}"
    ))
    .fetch_optional(pool)
    .await
    .map_err(err)
    .map(|opt| opt.map(Job::from))
}

pub async fn complete(pool: &PgPool, job_id: Uuid, result: JsonValue) -> Result<(), RuntimeError> {
    sqlx::query("UPDATE rootcx_system.jobs SET status = 'completed', result = $2, updated_at = now() WHERE id = $1")
        .bind(job_id)
        .bind(&result)
        .execute(pool)
        .await
        .map_err(err)?;
    Ok(())
}

pub async fn fail(pool: &PgPool, job_id: Uuid, error: &str) -> Result<(), RuntimeError> {
    sqlx::query("UPDATE rootcx_system.jobs SET status = 'failed', error = $2, updated_at = now() WHERE id = $1")
        .bind(job_id)
        .bind(error)
        .execute(pool)
        .await
        .map_err(err)?;
    Ok(())
}

pub async fn get(pool: &PgPool, job_id: Uuid) -> Result<Option<Job>, RuntimeError> {
    sqlx::query_as::<_, JobRow>(&format!("SELECT {COLS} FROM rootcx_system.jobs WHERE id = $1"))
        .bind(job_id)
        .fetch_optional(pool)
        .await
        .map_err(err)
        .map(|opt| opt.map(Job::from))
}

pub async fn list_for_app(
    pool: &PgPool,
    app_id: &str,
    status_filter: Option<&str>,
    limit: i64,
) -> Result<Vec<Job>, RuntimeError> {
    let base = format!("SELECT {COLS} FROM rootcx_system.jobs WHERE app_id = $1");
    let rows: Vec<JobRow> = match status_filter {
        Some(s) => {
            sqlx::query_as(&format!("{base} AND status = $2 ORDER BY created_at DESC LIMIT $3"))
                .bind(app_id)
                .bind(s)
                .bind(limit)
                .fetch_all(pool)
                .await
        }
        None => {
            sqlx::query_as(&format!("{base} ORDER BY created_at DESC LIMIT $2"))
                .bind(app_id)
                .bind(limit)
                .fetch_all(pool)
                .await
        }
    }
    .map_err(err)?;
    Ok(rows.into_iter().map(Job::from).collect())
}
