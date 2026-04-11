use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use crate::RuntimeError;
use crate::manifest::quote_literal;

static PG_CRON_AVAILABLE: AtomicBool = AtomicBool::new(false);

fn err(e: impl std::fmt::Display) -> RuntimeError { RuntimeError::Cron(e.to_string()) }

fn require_pg_cron() -> Result<(), RuntimeError> {
    if !PG_CRON_AVAILABLE.load(Ordering::Relaxed) {
        return Err(RuntimeError::Cron(
            "pg_cron not available — add shared_preload_libraries='pg_cron' to postgresql.conf and restart PostgreSQL".into()
        ));
    }
    Ok(())
}

pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    match sqlx::query("CREATE EXTENSION IF NOT EXISTS pg_cron").execute(pool).await {
        Ok(_) => PG_CRON_AVAILABLE.store(true, Ordering::Relaxed),
        Err(e) => {
            warn!("pg_cron not available — cron features disabled ({e})");
            return Ok(());
        }
    }

    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.cron_schedules (
            id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            app_id          TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
            name            TEXT NOT NULL,
            schedule        TEXT NOT NULL,
            timezone        TEXT,
            payload         JSONB NOT NULL DEFAULT '{}'::jsonb,
            overlap_policy  TEXT NOT NULL DEFAULT 'skip' CHECK (overlap_policy IN ('skip','queue')),
            enabled         BOOLEAN NOT NULL DEFAULT true,
            pg_job_id       BIGINT,
            created_by      UUID,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            UNIQUE (app_id, name)
        )
    "#).execute(pool).await.map_err(err)?;

    sqlx::query(
        "ALTER TABLE rootcx_system.cron_schedules ADD COLUMN IF NOT EXISTS created_by UUID"
    ).execute(pool).await.map_err(err)?;

    sqlx::query(r#"
        CREATE OR REPLACE FUNCTION rootcx_system.enqueue_cron(
            p_cron_id    uuid,
            p_app_id     text,
            p_payload    jsonb,
            p_skip_over  boolean
        ) RETURNS bigint
        LANGUAGE plpgsql AS $fn$
        DECLARE v_id bigint; v_user_id uuid; v_msg jsonb;
        BEGIN
            IF p_skip_over AND EXISTS (
                SELECT 1 FROM pgmq.q_jobs
                WHERE message->'payload'->>'cron_id' = p_cron_id::text
            ) THEN
                RETURN NULL;
            END IF;
            SELECT created_by INTO v_user_id
            FROM rootcx_system.cron_schedules WHERE id = p_cron_id;
            v_msg := jsonb_build_object(
                'app_id',  p_app_id,
                'payload', p_payload || jsonb_build_object('cron_id', p_cron_id::text)
            );
            IF v_user_id IS NOT NULL THEN
                v_msg := v_msg || jsonb_build_object('user_id', v_user_id::text);
            END IF;
            SELECT pgmq.send('jobs', v_msg) INTO v_id;
            RETURN v_id;
        END
        $fn$;
    "#).execute(pool).await.map_err(err)?;

    info!("pg_cron + cron_schedules ready");
    Ok(())
}

/// Deferred because cron_schedules bootstraps before RBAC creates the users table.
pub async fn add_deferred_constraints(pool: &PgPool) -> Result<(), RuntimeError> {
    if !PG_CRON_AVAILABLE.load(Ordering::Relaxed) { return Ok(()); }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_constraint WHERE conname = 'cron_schedules_created_by_fkey')"
    ).fetch_one(pool).await.map_err(err)?;
    if exists { return Ok(()); }
    sqlx::query(
        "ALTER TABLE rootcx_system.cron_schedules ADD CONSTRAINT cron_schedules_created_by_fkey \
         FOREIGN KEY (created_by) REFERENCES rootcx_system.users(id) ON DELETE SET NULL"
    ).execute(pool).await.map_err(err)?;
    Ok(())
}

// ── Types ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CronSchedule {
    pub id: Uuid,
    pub app_id: String,
    pub name: String,
    pub schedule: String,
    pub timezone: Option<String>,
    pub payload: JsonValue,
    pub overlap_policy: String,
    pub enabled: bool,
    pub pg_job_id: Option<i64>,
    pub created_by: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct CreateCron {
    pub name: String,
    pub schedule: String,
    pub timezone: Option<String>,
    pub payload: JsonValue,
    pub overlap_policy: String,
    pub created_by: Option<Uuid>,
}

const SELECT_COLS: &str =
    "id, app_id, name, schedule, timezone, payload, overlap_policy, enabled, pg_job_id, created_by, created_at::text AS created_at, updated_at::text AS updated_at";

// ── CRUD ────────────────────────────────────────────────────────────

pub async fn create(pool: &PgPool, app_id: &str, c: CreateCron) -> Result<CronSchedule, RuntimeError> {
    require_pg_cron()?;
    validate_name(&c.name)?;
    validate_schedule(&c.schedule)?;
    validate_overlap(&c.overlap_policy)?;

    let id = Uuid::new_v4();
    let skip = c.overlap_policy == "skip";

    let mut tx = pool.begin().await.map_err(err)?;

    sqlx::query(
        "INSERT INTO rootcx_system.cron_schedules (id, app_id, name, schedule, timezone, payload, overlap_policy, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)"
    )
    .bind(id).bind(app_id).bind(&c.name).bind(&c.schedule)
    .bind(&c.timezone).bind(&c.payload).bind(&c.overlap_policy).bind(c.created_by)
    .execute(&mut *tx).await.map_err(err)?;

    let pg_job_id = schedule_pg_cron(&mut tx, &id, app_id, &c.schedule, &c.payload, skip).await?;

    let row: CronSchedule = sqlx::query_as(&format!(
        "UPDATE rootcx_system.cron_schedules SET pg_job_id = $1 WHERE id = $2 RETURNING {SELECT_COLS}"
    )).bind(pg_job_id).bind(id).fetch_one(&mut *tx).await.map_err(err)?;

    tx.commit().await.map_err(err)?;
    info!(app_id, name = %c.name, "cron created");
    Ok(row)
}

pub async fn update(
    pool: &PgPool, app_id: &str, cron_id: Uuid,
    schedule: Option<&str>, payload: Option<&JsonValue>, overlap: Option<&str>, enabled: Option<bool>,
) -> Result<CronSchedule, RuntimeError> {
    require_pg_cron()?;
    if let Some(s) = schedule { validate_schedule(s)?; }
    if let Some(o) = overlap { validate_overlap(o)?; }

    let mut tx = pool.begin().await.map_err(err)?;

    // Lock the row inside the transaction to avoid TOCTOU
    let current: CronSchedule = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM rootcx_system.cron_schedules WHERE id = $1 AND app_id = $2 FOR UPDATE"
    )).bind(cron_id).bind(app_id).fetch_optional(&mut *tx).await.map_err(err)?
      .ok_or_else(|| RuntimeError::NotFound(format!("cron '{cron_id}' not found")))?;

    let new_schedule = schedule.unwrap_or(&current.schedule);
    let new_payload = payload.unwrap_or(&current.payload);
    let new_overlap = overlap.unwrap_or(&current.overlap_policy);
    let new_enabled = enabled.unwrap_or(current.enabled);

    if let Some(job_id) = current.pg_job_id {
        unschedule_pg_cron(&mut tx, job_id).await?;
    }

    let pg_job_id = if new_enabled {
        Some(schedule_pg_cron(&mut tx, &cron_id, app_id, new_schedule, new_payload, new_overlap == "skip").await?)
    } else {
        None
    };

    let row: CronSchedule = sqlx::query_as(&format!(
        "UPDATE rootcx_system.cron_schedules
         SET schedule = $1, payload = $2, overlap_policy = $3, enabled = $4, pg_job_id = $5, updated_at = now()
         WHERE id = $6 AND app_id = $7
         RETURNING {SELECT_COLS}"
    ))
    .bind(new_schedule).bind(new_payload).bind(new_overlap)
    .bind(new_enabled).bind(pg_job_id).bind(cron_id).bind(app_id)
    .fetch_one(&mut *tx).await.map_err(err)?;

    tx.commit().await.map_err(err)?;
    info!(app_id, id = %cron_id, "cron updated");
    Ok(row)
}

pub async fn delete(pool: &PgPool, app_id: &str, cron_id: Uuid) -> Result<(), RuntimeError> {
    require_pg_cron()?;
    let mut tx = pool.begin().await.map_err(err)?;

    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT pg_job_id FROM rootcx_system.cron_schedules WHERE id = $1 AND app_id = $2 FOR UPDATE"
    ).bind(cron_id).bind(app_id).fetch_optional(&mut *tx).await.map_err(err)?;

    let (pg_job_id,) = row.ok_or_else(|| RuntimeError::NotFound(format!("cron '{cron_id}' not found")))?;

    if let Some(jid) = pg_job_id {
        unschedule_pg_cron(&mut tx, jid).await?;
    }
    sqlx::query("DELETE FROM rootcx_system.cron_schedules WHERE id = $1 AND app_id = $2")
        .bind(cron_id).bind(app_id).execute(&mut *tx).await.map_err(err)?;

    tx.commit().await.map_err(err)?;
    info!(app_id, id = %cron_id, "cron deleted");
    Ok(())
}

pub async fn delete_all_for_app(pool: &PgPool, app_id: &str) -> Result<(), RuntimeError> {
    if !PG_CRON_AVAILABLE.load(Ordering::Relaxed) { return Ok(()); }
    let mut tx = pool.begin().await.map_err(err)?;

    let rows: Vec<(Option<i64>,)> = sqlx::query_as(
        "SELECT pg_job_id FROM rootcx_system.cron_schedules WHERE app_id = $1 FOR UPDATE"
    ).bind(app_id).fetch_all(&mut *tx).await.map_err(err)?;

    for (job_id,) in rows {
        if let Some(jid) = job_id {
            unschedule_pg_cron(&mut tx, jid).await?;
        }
    }
    sqlx::query("DELETE FROM rootcx_system.cron_schedules WHERE app_id = $1")
        .bind(app_id).execute(&mut *tx).await.map_err(err)?;

    tx.commit().await.map_err(err)?;
    Ok(())
}

pub async fn list(pool: &PgPool, app_id: &str) -> Result<Vec<CronSchedule>, RuntimeError> {
    if !PG_CRON_AVAILABLE.load(Ordering::Relaxed) { return Ok(vec![]); }
    sqlx::query_as::<_, CronSchedule>(&format!(
        "SELECT {SELECT_COLS} FROM rootcx_system.cron_schedules WHERE app_id = $1 ORDER BY name"
    )).bind(app_id).fetch_all(pool).await.map_err(err)
}

pub async fn get(pool: &PgPool, app_id: &str, cron_id: Uuid) -> Result<CronSchedule, RuntimeError> {
    sqlx::query_as::<_, CronSchedule>(&format!(
        "SELECT {SELECT_COLS} FROM rootcx_system.cron_schedules WHERE id = $1 AND app_id = $2"
    )).bind(cron_id).bind(app_id).fetch_optional(pool).await.map_err(err)?
      .ok_or_else(|| RuntimeError::NotFound(format!("cron '{cron_id}' not found")))
}

pub async fn trigger(pool: &PgPool, app_id: &str, cron_id: Uuid) -> Result<i64, RuntimeError> {
    require_pg_cron()?;
    let row = get(pool, app_id, cron_id).await?;
    let msg_id = crate::jobs::enqueue(pool, app_id, row.payload, row.created_by).await?;
    info!(app_id, id = %cron_id, msg_id, "cron triggered manually");
    Ok(msg_id)
}

// ── Manifest sync ───────────────────────────────────────────────────

pub async fn sync_from_manifest(
    pool: &PgPool,
    app_id: &str,
    defs: &[rootcx_types::CronDefinition],
    created_by: Option<Uuid>,
) -> Result<(), RuntimeError> {
    if !PG_CRON_AVAILABLE.load(Ordering::Relaxed) {
        warn!(app_id, "skipping cron sync — pg_cron not available");
        return Ok(());
    }
    let existing = list(pool, app_id).await?;
    let manifest_names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();

    for row in &existing {
        if !manifest_names.contains(&row.name.as_str()) {
            delete(pool, app_id, row.id).await?;
        }
    }

    for def in defs {
        let mut payload = def.payload.clone().unwrap_or(serde_json::json!({}));
        if let Some(ref method) = def.method {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("method".into(), serde_json::json!(method));
            } else {
                return Err(RuntimeError::Cron(format!(
                    "cron '{}': payload must be a JSON object", def.name
                )));
            }
        }

        match existing.iter().find(|r| r.name == def.name) {
            Some(row) => {
                update(
                    pool, app_id, row.id,
                    Some(&def.schedule), Some(&payload), Some(&def.overlap_policy), Some(true),
                ).await?;
            }
            None => {
                create(pool, app_id, CreateCron {
                    name: def.name.clone(),
                    schedule: def.schedule.clone(),
                    timezone: def.timezone.clone(),
                    payload,
                    overlap_policy: def.overlap_policy.clone(),
                    created_by,
                }).await?;
            }
        }
    }
    Ok(())
}

// ── pg_cron helpers ─────────────────────────────────────────────────

fn cron_command(cron_id: &Uuid, app_id: &str, payload: &JsonValue, skip: bool) -> String {
    format!(
        "SELECT rootcx_system.enqueue_cron({id}::uuid, {app}::text, {pay}::jsonb, {skip}::boolean)",
        id = quote_literal(&cron_id.to_string()),
        app = quote_literal(app_id),
        pay = quote_literal(&payload.to_string()),
        skip = skip,
    )
}

async fn schedule_pg_cron(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    cron_id: &Uuid, app_id: &str, schedule: &str, payload: &JsonValue, skip: bool,
) -> Result<i64, RuntimeError> {
    let jobname = format!("rootcx-{cron_id}");
    let command = cron_command(cron_id, app_id, payload, skip);
    let (job_id,): (i64,) = sqlx::query_as(
        "SELECT cron.schedule($1, $2, $3)"
    ).bind(&jobname).bind(schedule).bind(&command)
    .fetch_one(&mut **tx).await.map_err(err)?;
    Ok(job_id)
}

async fn unschedule_pg_cron(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job_id: i64,
) -> Result<(), RuntimeError> {
    sqlx::query("SELECT cron.unschedule($1)")
        .bind(job_id).execute(&mut **tx).await.map_err(err)?;
    Ok(())
}

// ── Validation ──────────────────────────────────────────────────────

fn validate_name(name: &str) -> Result<(), RuntimeError> {
    if name.is_empty() || name.len() > 64
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(RuntimeError::Cron(format!("invalid cron name: '{name}'")));
    }
    Ok(())
}

fn validate_schedule(schedule: &str) -> Result<(), RuntimeError> {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    // pg_cron interval syntax: "N seconds" (1-59)
    if parts.len() == 2 && parts[1] == "seconds" {
        let n: u32 = parts[0].parse().map_err(|_| RuntimeError::Cron(
            format!("invalid seconds interval: '{schedule}'")))?;
        if n < 1 || n > 59 {
            return Err(RuntimeError::Cron(format!("seconds must be 1-59, got: {n}")));
        }
        return Ok(());
    }
    if parts.len() != 5 {
        return Err(RuntimeError::Cron(format!(
            "invalid cron schedule: '{schedule}' (expected 5 fields or 'N seconds')"
        )));
    }
    if !schedule.bytes().all(|b| b.is_ascii_digit() || b == b' ' || b == b'*' || b == b'/' || b == b'-' || b == b',' || b == b'$') {
        return Err(RuntimeError::Cron(format!("invalid characters in cron schedule: '{schedule}'")));
    }
    Ok(())
}

fn validate_overlap(policy: &str) -> Result<(), RuntimeError> {
    if policy != "skip" && policy != "queue" {
        return Err(RuntimeError::Cron(format!("overlap_policy must be 'skip' or 'queue', got: '{policy}'")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn validate_name_accepts_valid() {
        for name in ["daily-sync", "sync_orders", "job1", "a", "A-b_c123"] {
            assert!(validate_name(name).is_ok(), "should accept: {name}");
        }
    }

    #[test]
    fn validate_name_rejects_invalid() {
        let long = "a".repeat(65);
        let cases: Vec<(&str, &str)> = vec![
            ("", "empty"),
            ("has space", "spaces"),
            ("semi;colon", "semicolon"),
            ("quo'te", "single quote"),
            ("back\\slash", "backslash"),
            ("🔥", "unicode emoji"),
            (&long, "65 chars"),
        ];
        for (name, label) in cases {
            assert!(validate_name(name).is_err(), "should reject {label}: {name:?}");
        }
    }

    #[test]
    fn validate_schedule_accepts_valid() {
        for expr in ["0 8 * * *", "*/5 * * * *", "0 0 1,15 * *", "30 2 * * 1-5", "0 0 1 1 *", "10 seconds", "1 seconds", "59 seconds", "0 0 $ * *"] {
            assert!(validate_schedule(expr).is_ok(), "should accept: {expr}");
        }
    }

    #[test]
    fn validate_schedule_rejects_invalid() {
        let cases: Vec<(&str, &str)> = vec![
            ("", "empty"),
            ("0 8 * *", "4 fields"),
            ("0 8 * * * *", "6 fields"),
            ("0 seconds", "zero seconds"),
            ("60 seconds", "60 seconds out of range"),
            ("abc seconds", "non-numeric seconds"),
            ("@daily", "named shortcut"),
            ("0 8 * * MON", "alpha day name"),
            ("; DROP TABLE x;--", "injection attempt"),
            ("0 8 * * * ; rm -rf /", "shell injection in extra field"),
        ];
        for (expr, label) in cases {
            assert!(validate_schedule(expr).is_err(), "should reject {label}: {expr:?}");
        }
    }

    #[test]
    fn cron_command_escapes_single_quotes() {
        let id = Uuid::nil();
        let cmd = cron_command(&id, "my_app", &json!({"key": "it's"}), true);

        // app_id and payload are embedded as SQL literals — verify no unescaped quotes
        assert!(cmd.contains("'my_app'"), "app_id should be quoted: {cmd}");
        assert!(!cmd.contains("it's}"), "single quote in payload must be doubled: {cmd}");
        assert!(cmd.contains("it''s"), "single quote should be escaped: {cmd}");
        assert!(cmd.contains("true::boolean"), "skip=true: {cmd}");
    }

    #[test]
    fn cron_command_hostile_app_id() {
        let id = Uuid::nil();
        let hostile = "x'; DROP TABLE rootcx_system.apps;--";
        let cmd = cron_command(&id, hostile, &json!({}), false);

        // The single quote in app_id must be doubled, so the literal doesn't terminate early
        assert!(cmd.contains("x''; DROP TABLE"), "quote must be escaped: {cmd}");
        // Entire hostile string wrapped in one literal pair
        let literal_start = cmd.find("x''").unwrap();
        let before = &cmd[..literal_start];
        assert!(before.ends_with('\''), "literal must start with quote: {cmd}");
    }

    #[test]
    fn cron_command_structure() {
        let id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let cmd = cron_command(&id, "shop", &json!({"method": "sync"}), false);

        assert!(cmd.starts_with("SELECT rootcx_system.enqueue_cron("));
        assert!(cmd.contains("::uuid"));
        assert!(cmd.contains("::text"));
        assert!(cmd.contains("::jsonb"));
        assert!(cmd.contains("false::boolean"));
        assert!(cmd.contains("550e8400"));
    }
}
