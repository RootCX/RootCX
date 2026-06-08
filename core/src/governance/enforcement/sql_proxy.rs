//! SQL proxy: the single data path from an untrusted app to Postgres.
//!
//! Apps never hold a DB connection. They send SQL over IPC; the core executes
//! it inside a transaction that (1) scopes the search_path to the app schema
//! (never `rootcx_system`), (2) poses the three RLS identity GUCs, and (3)
//! drops to the non-superuser `rootcx_app_executor` role before running the
//! statement. RLS — not the app — decides what rows are visible.

use serde_json::Value as JsonValue;
use sqlx::postgres::PgColumn;
use sqlx::{Column, PgPool, Row as _};
use uuid::Uuid;

use crate::manifest::quote_ident;
use crate::routes::introspection::pg_val;

const MAX_ROWS: usize = 1_000;

/// Timeout tiers (milliseconds). Postgres cancels the statement at the limit.
/// - INTERACTIVE: ctx.sql, HTTP CRUD, worker collection ops (user-facing, fast)
/// - AGENT_TOOL: AI agent tool calls (complex joins, larger scans)
/// Citation: Supabase uses 8s for API, 60s for functions; PostgREST default 10s.
/// We use 8s/30s to match Supabase API/function pattern.
pub const TIMEOUT_INTERACTIVE_MS: u32 = 8_000;
pub const TIMEOUT_AGENT_TOOL_MS: u32 = 30_000;

/// Resolved identity for a unit of work. The core binds this to a worker's
/// sole in-flight unit out-of-band; it is never carried on a worker-controlled
/// message, so an untrusted worker cannot select another user's identity.
#[derive(Debug, Clone, Default)]
pub struct ContextState {
    pub user_id: Option<Uuid>,
    pub is_delegated: bool,
    pub effective_perms: Vec<String>,
}

impl ContextState {
    /// Build from an IPC caller: a delegated caller carries `effective_perms`.
    pub fn from_caller(caller: Option<&crate::ipc::RpcCaller>) -> Self {
        match caller {
            Some(c) => Self {
                user_id: c.user_id.parse().ok(),
                is_delegated: c.effective_perms.is_some(),
                effective_perms: c.effective_perms.clone().unwrap_or_default(),
            },
            None => Self::default(),
        }
    }
}

/// Pose the three RLS identity GUCs for the open transaction. MUST run before
/// `SET LOCAL ROLE rootcx_app_executor` — the executor cannot call `set_config`
/// (revoked), so the app can never rewrite its own identity.
pub async fn set_rls_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    state: &ContextState,
) -> Result<(), sqlx::Error> {
    let uid = state.user_id.map(|u| u.to_string()).unwrap_or_default();
    let delegated = if state.is_delegated { "1" } else { "0" };
    let perms = state.effective_perms.join(",");
    sqlx::query(
        "SELECT set_config('rootcx.user_id', $1, true), \
                set_config('rootcx.is_delegated', $2, true), \
                set_config('rootcx.effective_perms', $3, true)",
    )
    .bind(uid)
    .bind(delegated)
    .bind(perms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Open a transaction primed for RLS-governed app access: scoped search_path,
/// the RLS identity GUCs, the audit attribution GUCs, statement_timeout,
/// idle_in_transaction_session_timeout, then a drop to the non-superuser
/// executor role. Every SET LOCAL runs while still superuser (the executor has
/// set_config revoked). Callers run their statements on the returned tx and
/// commit.
pub async fn begin_app_tx<'a>(
    pool: &'a PgPool,
    app_schema: &str,
    state: &ContextState,
    audit_actor: Option<Uuid>,
    audit_delegator: Option<Uuid>,
    trigger_ref: &str,
    timeout_ms: u32,
) -> Result<sqlx::Transaction<'a, sqlx::Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(&format!("SET LOCAL search_path TO {}, public", quote_ident(app_schema)))
        .execute(&mut *tx).await?;
    // Timeout + zombie tx protection. SET LOCAL scopes to this tx only.
    sqlx::query(&format!("SET LOCAL statement_timeout = '{timeout_ms}'"))
        .execute(&mut *tx).await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout = '30000'")
        .execute(&mut *tx).await?;
    set_rls_context(&mut tx, state).await?;
    crate::extensions::audit::set_context(&mut tx, audit_actor, audit_delegator, trigger_ref).await?;
    sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await?;
    Ok(tx)
}

/// Best-effort, early rejection of obvious DDL / privileged statements so apps
/// get a clear error instead of a raw permission failure. This is NOT the
/// security boundary: multi-statement is blocked structurally by sqlx's extended
/// query protocol, and the `rootcx_app_executor` role has no DDL, no `DO`, and
/// no `set_config`. A real query never starts with these keywords, so there are
/// no false positives.
const BLOCKED_PREFIXES: &[&str] =
    &["CREATE", "DROP", "ALTER", "TRUNCATE", "GRANT", "REVOKE", "REINDEX", "VACUUM", "COPY", "SET", "RESET", "DO"];

pub fn validate_sql(sql: &str) -> Result<(), String> {
    let head = sql.trim_start().to_ascii_uppercase();
    for kw in BLOCKED_PREFIXES {
        if head.starts_with(kw) {
            let rest = &head[kw.len()..];
            // Match keyword alone or followed by whitespace/dollar (DO$$...)
            if rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace()) || rest.starts_with('$') {
                return Err(format!("statement not allowed: {kw}"));
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct SqlOk {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<JsonValue>>,
    pub row_count: usize,
}

/// Build typed PgArguments from JSON params using PG's inferred parameter types.
/// Calls `describe` on the connection to learn what each `$N` expects, then binds
/// the JSON values with the correct Rust type. Cached by sqlx per SQL string, so
/// only the first call per unique query pays a Describe round-trip.
pub async fn build_typed_args(
    conn: &mut sqlx::PgConnection,
    sql: &str,
    params: &[JsonValue],
) -> Result<sqlx::postgres::PgArguments, String> {
    use sqlx::postgres::PgArguments;
    use sqlx::{Executor, TypeInfo};

    let desc = conn.describe(sql).await.map_err(|e| format!("describe: {e}"))?;
    let pg_types: &[_] = match desc.parameters() {
        Some(either::Either::Left(ref types)) => types,
        _ => &[],
    };

    let mut args = PgArguments::default();
    for (i, value) in params.iter().enumerate() {
        let type_name = pg_types.get(i).map(|t| t.name()).unwrap_or("TEXT");
        bind_typed_value(&mut args, value, type_name)
            .map_err(|e| format!("param ${}: {e}", i + 1))?;
    }
    Ok(args)
}

fn bind_typed_value(
    args: &mut sqlx::postgres::PgArguments,
    value: &JsonValue,
    type_name: &str,
) -> Result<(), String> {
    use sqlx::Arguments;

    if value.is_null() {
        args.add(Option::<String>::None).map_err(|e| e.to_string())?;
        return Ok(());
    }

    match type_name {
        "BOOL" => {
            let v = value.as_bool().ok_or("expected bool")?;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "INT2" => {
            let v = value.as_i64().ok_or("expected integer")? as i16;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "INT4" => {
            let v = value.as_i64().ok_or("expected integer")? as i32;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "INT8" => {
            let v = value.as_i64().ok_or("expected integer")?;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "FLOAT4" => {
            let v = value.as_f64().ok_or("expected number")? as f32;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "FLOAT8" => {
            let v = value.as_f64().ok_or("expected number")?;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "UUID" => {
            let s = value.as_str().ok_or("expected string for uuid")?;
            let v: uuid::Uuid = s.parse().map_err(|e| format!("invalid uuid: {e}"))?;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "TIMESTAMPTZ" => {
            let s = value.as_str().ok_or("expected string for timestamptz")?;
            let v: chrono::DateTime<chrono::Utc> = s.parse().map_err(|e| format!("invalid timestamptz: {e}"))?;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "TIMESTAMP" => {
            let s = value.as_str().ok_or("expected string for timestamp")?;
            let v: chrono::NaiveDateTime = s.parse().map_err(|e| format!("invalid timestamp: {e}"))?;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "DATE" => {
            let s = value.as_str().ok_or("expected string for date")?;
            let v: chrono::NaiveDate = s.parse().map_err(|e| format!("invalid date: {e}"))?;
            args.add(v).map_err(|e| e.to_string())?;
        }
        "JSONB" | "JSON" => {
            args.add(sqlx::types::Json(value.clone())).map_err(|e| e.to_string())?;
        }
        "TEXT[]" => {
            let arr: Vec<String> = match value {
                JsonValue::Array(a) => a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect(),
                _ => return Err("expected array for TEXT[]".into()),
            };
            args.add(arr).map_err(|e| e.to_string())?;
        }
        "UUID[]" => {
            let arr: Vec<uuid::Uuid> = match value {
                JsonValue::Array(a) => a.iter().map(|v| {
                    v.as_str().unwrap_or("").parse::<uuid::Uuid>()
                }).collect::<Result<_, _>>().map_err(|e| format!("invalid uuid in array: {e}"))?,
                _ => return Err("expected array for UUID[]".into()),
            };
            args.add(arr).map_err(|e| e.to_string())?;
        }
        // Fallback: bind as text — works for TEXT, VARCHAR, and many types that
        // accept text input format.
        _ => {
            let s = value.as_str().map(|s| s.to_string()).unwrap_or_else(|| value.to_string());
            args.add(s).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}


/// Serialize PG rows to JSON with a row-count cap. Returns columns + rows or
/// an error if the cap is exceeded.
pub fn serialize_rows(rows: Vec<sqlx::postgres::PgRow>) -> Result<SqlOk, String> {
    if rows.is_empty() {
        return Ok(SqlOk { columns: vec![], rows: vec![], row_count: 0 });
    }
    if rows.len() > MAX_ROWS {
        return Err(format!("query returned {} rows, exceeds limit {MAX_ROWS}; add LIMIT or paginate", rows.len()));
    }
    let columns: Vec<String> = rows[0].columns().iter().map(|c: &PgColumn| c.name().to_string()).collect();
    let json_rows: Vec<Vec<JsonValue>> = rows
        .iter()
        .map(|row| row.columns().iter().enumerate().map(|(i, col)| pg_val(row, i, col.type_info())).collect())
        .collect();
    Ok(SqlOk { row_count: json_rows.len(), columns, rows: json_rows })
}

/// Execute one app statement under RLS. `app_schema` is a validated snake_case
/// identifier. Returns rows (RETURNING / SELECT) or an empty set for plain DML.
pub async fn run_sql(
    pool: &PgPool,
    app_schema: &str,
    state: &ContextState,
    sql: &str,
    params: &[JsonValue],
) -> Result<SqlOk, String> {
    validate_sql(sql)?;

    let mut tx = begin_app_tx(pool, app_schema, state, state.user_id, None, "app_sql", TIMEOUT_INTERACTIVE_MS)
        .await.map_err(|e| e.to_string())?;

    let args = build_typed_args(&mut *tx, sql, params).await?;
    let rows = sqlx::query_with(sql, args).fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;

    let result = serialize_rows(rows);
    match &result {
        Ok(_) => { tx.commit().await.map_err(|e| e.to_string())?; }
        Err(_) => { let _ = tx.rollback().await; }
    }
    result
}

// ── Multi-statement transaction session ─────────────────────────────

use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio::time::{sleep_until, Instant as TokioInstant};

/// Absolute wall-time budget for an entire transaction (begin → commit). Bounds
/// TOTAL lifetime, independent of the per-statement `statement_timeout` (8s) and
/// the between-statement `idle_in_transaction_session_timeout` (30s) already
/// posed by `begin_app_tx`. Without it, an app could pace statements to keep a
/// transaction (and its pooled connection) alive forever.
const TX_MAX_WALL_TIME: Duration = Duration::from_secs(60);

/// Process-global cap on concurrent held-open app transactions. Held strictly
/// below the pool's `max_connections` so transactions can never starve the
/// auto-commit (`run_sql`), HTTP CRUD, or agent paths that share the pool — the
/// assert below makes the relationship a build break, not a comment.
const TX_MAX_CONCURRENT: usize = 8;
const _: () = assert!(TX_MAX_CONCURRENT < crate::POOL_MAX_CONNECTIONS as usize);
static TX_SEMAPHORE: Semaphore = Semaphore::const_new(TX_MAX_CONCURRENT);

enum TxCmd {
    Exec { sql: String, params: Vec<JsonValue>, reply: oneshot::Sender<Result<SqlOk, String>> },
    Commit { reply: oneshot::Sender<Result<(), String>> },
    Rollback { reply: oneshot::Sender<Result<(), String>> },
}

const TX_GONE: &str = "transaction no longer active";
pub const TX_NONE: &str = "no open transaction";
pub const TX_MISMATCH: &str = "tx_id mismatch";

async fn round_trip<R>(
    cmd_tx: &mpsc::Sender<TxCmd>,
    make: impl FnOnce(oneshot::Sender<Result<R, String>>) -> TxCmd,
) -> Result<R, String> {
    let (reply_tx, reply_rx) = oneshot::channel();
    cmd_tx.send(make(reply_tx)).await.map_err(|_| TX_GONE.to_string())?;
    reply_rx.await.unwrap_or_else(|_| Err(TX_GONE.into()))
}

/// Cloneable handle to send statements to an open transaction's task without
/// borrowing the session. Lets the supervisor spawn an exec round-trip instead
/// of awaiting it inline (which would head-of-line-block the worker's loop).
#[derive(Clone)]
pub struct TxExec {
    cmd_tx: mpsc::Sender<TxCmd>,
}

impl TxExec {
    pub async fn exec(&self, sql: String, params: Vec<JsonValue>) -> Result<SqlOk, String> {
        round_trip(&self.cmd_tx, |reply| TxCmd::Exec { sql, params, reply }).await
    }
}

/// Handle to a governed multi-statement transaction running on its own task.
///
/// The task owns the PG transaction AND the semaphore permit, and self-
/// terminates on the first of: commit, rollback, the wall-time deadline, or this
/// handle being dropped (worker crash/stop closes the command channel). On exit
/// it reports its `tx_id` on the `done` channel so the supervisor can clear its
/// slot. The pooled connection and the TX permit are therefore ALWAYS released
/// within `TX_MAX_WALL_TIME`, no matter how the worker dies — there is no path
/// that leaks them.
pub struct TxSession {
    pub tx_id: String,
    cmd_tx: mpsc::Sender<TxCmd>,
}

impl TxSession {
    /// Open a new governed transaction. Awaits until the TX is ready (or fails).
    /// `done` receives this session's `tx_id` when its task exits, for whatever
    /// reason — the supervisor's single source of truth for slot cleanup.
    pub async fn begin(
        pool: &PgPool,
        app_schema: &str,
        state: &ContextState,
        done: mpsc::Sender<String>,
    ) -> Result<Self, String> {
        // Fail fast when all TX slots are taken — never queue (would hold the
        // caller hostage) and never exceed the pool budget. The permit is moved
        // into the task and released only when the task exits.
        let permit = TX_SEMAPHORE
            .try_acquire()
            .map_err(|_| "too many concurrent transactions; retry".to_string())?;

        let pool = pool.clone();
        let app_schema = app_schema.to_string();
        let state = state.clone();
        let tx_id = uuid::Uuid::new_v4().to_string();
        let task_tx_id = tx_id.clone();
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<TxCmd>(8);
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), String>>();

        tokio::spawn(async move {
            let _permit = permit; // released on task exit → frees the TX slot

            // Inner scope so `pg_tx` is fully finalized (committed, rolled back,
            // or dropped → implicit rollback) BEFORE we signal `done`. A received
            // `done` therefore guarantees the connection is returned on every
            // path — including the channel-closed (worker crash/stop) path.
            {
                let mut pg_tx = match begin_app_tx(
                    &pool, &app_schema, &state, state.user_id, None, "app_tx", TIMEOUT_INTERACTIVE_MS,
                ).await {
                    Ok(t) => { let _ = ready_tx.send(Ok(())); t }
                    Err(e) => { let _ = ready_tx.send(Err(e.to_string())); return; }
                };

                let deadline = TokioInstant::now() + TX_MAX_WALL_TIME;
                loop {
                    tokio::select! {
                        biased;
                        _ = sleep_until(deadline) => { let _ = pg_tx.rollback().await; break; }
                        cmd = cmd_rx.recv() => match cmd {
                            Some(TxCmd::Exec { sql, params, reply }) => {
                                let result = match validate_sql(&sql) {
                                    Err(e) => Err(e),
                                    Ok(()) => match build_typed_args(&mut *pg_tx, &sql, &params).await {
                                        Err(e) => Err(e),
                                        Ok(args) => match sqlx::query_with(&sql, args)
                                            .fetch_all(&mut *pg_tx).await
                                        {
                                            Err(e) => Err(e.to_string()),
                                            Ok(rows) => serialize_rows(rows),
                                        },
                                    },
                                };
                                let _ = reply.send(result);
                            }
                            Some(TxCmd::Commit { reply }) => {
                                let _ = reply.send(pg_tx.commit().await.map_err(|e| e.to_string()));
                                break;
                            }
                            Some(TxCmd::Rollback { reply }) => {
                                let _ = reply.send(pg_tx.rollback().await.map_err(|e| e.to_string()));
                                break;
                            }
                            // Channel closed (handle dropped on worker crash/stop):
                            // pg_tx drops at scope end → implicit rollback.
                            None => break,
                        }
                    }
                }
            }
            let _ = done.send(task_tx_id).await;
        });

        match ready_rx.await {
            Ok(Ok(())) => Ok(Self { tx_id, cmd_tx }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("begin_app_tx task died".into()),
        }
    }

    /// A cloneable exec handle the supervisor can move into a spawned task.
    pub fn executor(&self) -> TxExec {
        TxExec { cmd_tx: self.cmd_tx.clone() }
    }

    /// Construct a handle with no live task, for unit-testing pure consumers
    /// (e.g. the supervisor's tx_id-matching guard). Never opens a DB tx.
    #[cfg(test)]
    pub(crate) fn dummy(tx_id: &str) -> Self {
        let (cmd_tx, _rx) = mpsc::channel(1);
        Self { tx_id: tx_id.to_string(), cmd_tx }
    }

    pub async fn commit(self) -> Result<(), String> {
        round_trip(&self.cmd_tx, |reply| TxCmd::Commit { reply }).await
    }

    pub async fn rollback(self) -> Result<(), String> {
        round_trip(&self.cmd_tx, |reply| TxCmd::Rollback { reply }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ddl_prefixes() {
        // Multi-statement is NOT checked here — sqlx's extended protocol blocks
        // it structurally. validate_sql only catches obvious DDL/privileged
        // statements early for a clearer error.
        for bad in [
            "CREATE TABLE x(id int)",
            "drop table contacts",
            "ALTER TABLE x ADD c int",
            "TRUNCATE contacts",
            "DO $$ BEGIN PERFORM 1; END $$",
            "DO$$BEGIN PERFORM 1; END$$",
            "SET ROLE rootcx_owner",
            "SET\tLOCAL statement_timeout = '0'",
            "SET\nROLE postgres",
            "RESET ROLE",
        ] {
            assert!(validate_sql(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn allows_normal_dml_with_no_false_positives() {
        for ok in [
            "SELECT * FROM contacts",
            "INSERT INTO contacts (name) VALUES ($1) RETURNING id",
            "UPDATE contacts SET name = $1 WHERE id = $2",
            "DELETE FROM contacts WHERE id = $1",
            "WITH c AS (SELECT 1) SELECT * FROM c",
            "SELECT * FROM t WHERE name = 'a;b'",     // ';' in a literal: not our concern
            "SELECT ';' AS x FROM t",                  // and never a false positive
            "SELECT * FROM settings WHERE key = $1",   // "SET" prefix in table name
            "SELECT * FROM resets",                     // "RESET" prefix in table name
        ] {
            assert!(validate_sql(ok).is_ok(), "should allow: {ok}");
        }
    }
}
