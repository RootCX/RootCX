//! SQL proxy: the single data path from an untrusted app to Postgres.
//!
//! Apps never hold a DB connection. They send SQL over IPC; the core executes
//! it inside a transaction that (1) scopes the search_path to the app schema
//! (never `rootcx_system`), (2) poses the RLS identity GUCs, and (3)
//! drops to the non-superuser `rootcx_app_executor` role before running the
//! statement. RLS — not the app — decides what rows are visible.

use serde_json::Value as JsonValue;
use sqlx::postgres::PgColumn;
use sqlx::{Column, Executor as _, PgPool, Row as _};
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
    /// Pinned integration connection for the worker's lifetime. When set, all
    /// credential resolution uses this connection instead of the created_at
    /// fallback. Inherited through sub-calls (ctx.action re-entries).
    pub connection_id: Option<String>,
    /// Audit attribution is distinct from the RLS owner. In a delegated worker
    /// the responsible human owns rows while the app/agent is the actor.
    pub audit_actor_id: Option<Uuid>,
    pub audit_delegator_id: Option<Uuid>,
}

/// The unit of work a statement belongs to, attached by Core at an execution
/// boundary. Unlike the user identity, this value is never accepted from
/// application SQL or IPC.
///
/// There is no `Workflow` variant and there never was: a scope is an action or a
/// job. An earlier `is_workflow()` spelling of [`Self::is_scoped`] implied a rare
/// new execution path and was used to refuse workers that could not carry an
/// invocation id, which in fact refused every declared action and every cron on
/// every worker written before the id existed. Name it for what it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum InvocationContext {
    #[default]
    None,
    Action(String),
    Job(String),
}

impl InvocationContext {
    pub fn action(action_id: &str) -> Self {
        Self::Action(action_id.into())
    }

    pub fn job(job_name: &str) -> Self {
        Self::Job(job_name.into())
    }

    /// Whether this unit of work carries a scope at all, i.e. is an action or a
    /// job rather than a bare call.
    pub fn is_scoped(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn key(&self) -> String {
        let (kind, name, _) = self.guc_values();
        format!("{kind}:{name}")
    }

    fn guc_values(&self) -> (&str, &str, &str) {
        match self {
            Self::None => ("", "", ""),
            Self::Action(action_id) => ("action", action_id, action_id),
            Self::Job(job_name) => ("job", job_name, ""),
        }
    }
}

impl ContextState {
    /// Build from an IPC caller: a delegated caller carries `effective_perms`.
    pub fn from_caller(caller: Option<&crate::ipc::RpcCaller>) -> Self {
        match caller {
            Some(c) => Self {
                user_id: c.user_id.parse().ok(),
                is_delegated: c.effective_perms.is_some(),
                effective_perms: c.effective_perms.clone().unwrap_or_default(),
                connection_id: c.connection_id.clone(),
                audit_actor_id: if c.effective_perms.is_none() { c.user_id.parse().ok() } else { None },
                audit_delegator_id: None,
            },
            None => Self::default(),
        }
    }
}

/// Pose the RLS identity GUCs for the open transaction. MUST run before
/// `SET LOCAL ROLE rootcx_app_executor` — the executor cannot call `set_config`
/// (revoked), so the app can never rewrite its own identity.
///
/// `rootcx.app_id` names the app this unit of work belongs to. The identity GUCs
/// say who the caller is; this one says on whose behalf the SQL runs, which is what
/// lets the generated ownership resolvers refuse an app that is not their own (see
/// `rbac::declare_owner_resolver`).
pub async fn set_rls_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    app_schema: &str,
    state: &ContextState,
) -> Result<(), sqlx::Error> {
    let uid = state.user_id.map(|u| u.to_string()).unwrap_or_default();
    let delegated = if state.is_delegated { "1" } else { "0" };
    let perms = state.effective_perms.join(",");
    sqlx::query(
        "SELECT set_config('rootcx.user_id', $1, true), \
                set_config('rootcx.is_delegated', $2, true), \
                set_config('rootcx.effective_perms', $3, true), \
                set_config('rootcx.app_id', $4, true)",
    )
    .bind(uid)
    .bind(delegated)
    .bind(perms)
    .bind(app_schema)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn set_invocation_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    invocation: &InvocationContext,
) -> Result<(), sqlx::Error> {
    let (kind, name, action_id) = invocation.guc_values();
    sqlx::query(
        "SELECT set_config('rootcx.invocation_kind', $1, true), \
                set_config('rootcx.invocation_name', $2, true), \
                set_config('rootcx.action_id', $3, true)",
    )
    .bind(kind)
    .bind(name)
    .bind(action_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Open a transaction primed for RLS-governed app access: read committed
/// isolation, scoped search_path, the RLS identity GUCs, the audit attribution
/// GUCs, statement_timeout, idle_in_transaction_session_timeout, then a drop to the non-superuser
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
    begin_app_tx_with_invocation(
        pool,
        app_schema,
        state,
        &InvocationContext::default(),
        audit_actor,
        audit_delegator,
        trigger_ref,
        timeout_ms,
    )
    .await
}

pub async fn begin_app_tx_with_invocation<'a>(
    pool: &'a PgPool,
    app_schema: &str,
    state: &ContextState,
    invocation: &InvocationContext,
    audit_actor: Option<Uuid>,
    audit_delegator: Option<Uuid>,
    trigger_ref: &str,
    timeout_ms: u32,
) -> Result<sqlx::Transaction<'a, sqlx::Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    // One round-trip; SET LOCAL scopes every value to this tx only.
    // `transaction_isolation` leads because PostgreSQL refuses it once the tx has
    // run a query — and `set_rls_context` below is a `SELECT`. Pinning it to read
    // committed is a governance requirement, not a perf choice: under repeatable
    // read the tx holds one frozen snapshot, so revoking access mid-transaction
    // (deleting an ownership row) would stay invisible for the tx's whole life.
    // Read committed takes a fresh snapshot per statement, so revocation lands on
    // the next statement.
    let batch = format!(
        "SET LOCAL transaction_isolation = 'read committed'; \
         SET LOCAL search_path TO {}, public; \
         SET LOCAL statement_timeout = '{timeout_ms}'; \
         SET LOCAL idle_in_transaction_session_timeout = '30000'",
        quote_ident(app_schema)
    );
    tx.execute(sqlx::raw_sql(&batch)).await?;
    set_rls_context(&mut tx, app_schema, state).await?;
    set_invocation_context(&mut tx, invocation).await?;
    crate::extensions::audit::set_context(&mut tx, audit_actor, audit_delegator, trigger_ref).await?;
    sqlx::query("SET LOCAL ROLE rootcx_app_executor").execute(&mut *tx).await?;
    Ok(tx)
}

/// Statements an app may send. Anything else is refused.
///
/// A leading `(` is also accepted: `(SELECT ...) UNION (SELECT ...)` is legal SQL.
const ALLOWED_PREFIXES: &[&str] =
    &["SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "VALUES", "TABLE"];

/// THIS IS A SECURITY BOUNDARY for privileged *utility* statements. Nothing else
/// in the stack stops them, because `SET` / `RESET` are not privilege-checked
/// operations — they need no grant, so role-level revocation cannot cover them.
/// Two attack shapes are refused here and nowhere else:
///
/// 1. **Role escape.** `RESET ROLE` / `SET ROLE <login role>` undoes the
///    `SET LOCAL ROLE rootcx_app_executor` in `begin_app_tx`, returning the
///    session to the pool's login role, which bootstrap requires to be SUPERUSER
///    or BYPASSRLS. Every app's RLS plus `rootcx_system` (secrets, RBAC,
///    credentials) is then readable.
/// 2. **Identity forgery.** `SET rootcx.user_id = '<victim>'` rewrites the GUC
///    that RLS reads as the caller identity. This needs no superuser at all:
///    the policies consult nothing but those GUCs. The `set_config()` route is
///    already closed (the function is revoked from `rootcx_app_executor`), but
///    bare `SET` is not a function call and is not revocable.
///
/// Both survive inside a held-open transaction (`TxSession`): statement 1 escapes,
/// statement 2 reads with the escaped privileges, because they share one session
/// and one `SET LOCAL` scope. Prefix *blocklisting* cannot express this — SQL
/// comments precede the keyword and `trim_start()` does not remove them, so
/// `/*x*/RESET ROLE` slips a blocklist. Hence: strip comments, then allowlist.
///
/// What the other layers do cover: sqlx's extended query protocol prevents a
/// second statement being smuggled into one `Exec` (so this need only classify a
/// single statement), and `rootcx_app_executor` lacks DDL, `DO`, and `set_config`.
/// What they do NOT cover: the two shapes above, and any future utility statement.
///
/// `EXPLAIN` is deliberately absent. `EXPLAIN (ANALYZE)` reports
/// `Rows Removed by Filter` — an exact count of rows RLS hid from the caller —
/// and planner estimates for tables the caller cannot read. It is an oracle over
/// invisible data, so it stays rejected.
pub fn validate_sql(sql: &str) -> Result<(), String> {
    let head = strip_leading_noise(sql)?;
    if head.starts_with('(') {
        return Ok(());
    }
    let upper = head.to_ascii_uppercase();
    let allowed = ALLOWED_PREFIXES.iter().any(|kw| {
        upper.strip_prefix(*kw).is_some_and(|rest| {
            rest.is_empty() || !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
        })
    });
    if allowed {
        return Ok(());
    }
    // Name the offending token so the app author sees what was refused, capped
    // because the rest of the statement is theirs and may be long.
    let shown: String = head.split_whitespace().next().unwrap_or("(empty)").chars().take(16).collect();
    Err(format!("statement not allowed: {shown}; apps may only send {}", ALLOWED_PREFIXES.join(", ")))
}

/// Skip leading whitespace and SQL comments until a real token starts. Block
/// comments nest in PostgreSQL, so `/*a/*b*/c*/` is one comment. An unterminated
/// comment is an error, not a scan past the end.
fn strip_leading_noise(sql: &str) -> Result<&str, String> {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            // Ends on CR *or* LF, as PostgreSQL's scanner does. Taking only LF
            // would let `--\rRESET ROLE\nSELECT 1` read as `SELECT 1` here while
            // the server reads `RESET ROLE` — harmless today only because that is
            // then two statements and Parse refuses those, which is not a property
            // this classifier should depend on. Unterminated runs to end of input.
            rest = after.find(['\n', '\r']).map_or("", |i| &after[i + 1..]).trim_start();
        } else if rest.starts_with("/*") {
            rest = skip_block_comment(rest)?.trim_start();
        } else {
            return Ok(rest);
        }
    }
}

fn skip_block_comment(s: &str) -> Result<&str, String> {
    let bytes = s.as_bytes();
    let mut i = 2;
    let mut depth = 1usize;
    while i + 1 < bytes.len() {
        match (bytes[i], bytes[i + 1]) {
            (b'/', b'*') => { depth += 1; i += 2; }
            (b'*', b'/') => {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    return Ok(&s[i..]);
                }
            }
            _ => i += 1,
        }
    }
    Err("statement not allowed: unterminated block comment".into())
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
        if type_name == "NUMERIC" {
            args.add(Option::<sqlx::types::BigDecimal>::None)
                .map_err(|e| e.to_string())?;
        } else {
            args.add(Option::<String>::None)
                .map_err(|e| e.to_string())?;
        }
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
        "NUMERIC" => {
            let literal = value.as_str().ok_or("expected string for exact decimal")?;
            let decimal = literal
                .parse::<sqlx::types::BigDecimal>()
                .map_err(|_| format!("invalid decimal: '{literal}'"))?;
            args.add(decimal).map_err(|e| e.to_string())?;
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
    run_sql_with_invocation(
        pool,
        app_schema,
        state,
        &InvocationContext::default(),
        sql,
        params,
    )
    .await
}

pub async fn run_sql_with_invocation(
    pool: &PgPool,
    app_schema: &str,
    state: &ContextState,
    invocation: &InvocationContext,
    sql: &str,
    params: &[JsonValue],
) -> Result<SqlOk, String> {
    validate_sql(sql)?;

    let mut tx = begin_app_tx_with_invocation(
        pool, app_schema, state, invocation, state.audit_actor_id,
        state.audit_delegator_id, "app_sql", TIMEOUT_INTERACTIVE_MS,
    )
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
    Poison { error: String, reply: oneshot::Sender<Result<(), String>> },
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

fn try_enqueue<R>(
    cmd_tx: &mpsc::Sender<TxCmd>,
    make: impl FnOnce(oneshot::Sender<Result<R, String>>) -> TxCmd,
) -> Result<oneshot::Receiver<Result<R, String>>, String> {
    let (reply, receive) = oneshot::channel();
    cmd_tx
        .try_send(make(reply))
        .map_err(|e| format!("transaction command queue unavailable: {e}"))?;
    Ok(receive)
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

    /// Enqueue without yielding so commands preserve the worker IPC order even
    /// when their replies are awaited by separate tasks.
    pub fn enqueue_exec(
        &self,
        sql: String,
        params: Vec<JsonValue>,
    ) -> Result<oneshot::Receiver<Result<SqlOk, String>>, String> {
        try_enqueue(&self.cmd_tx, |reply| TxCmd::Exec { sql, params, reply })
    }

    /// Poison a transaction for an execution error detected before PostgreSQL
    /// (for example the worker's SQL rate limit). A poisoned transaction can
    /// only roll back; commit will fail closed.
    pub fn enqueue_poison(
        &self,
        error: String,
    ) -> Result<oneshot::Receiver<Result<(), String>>, String> {
        try_enqueue(&self.cmd_tx, |reply| TxCmd::Poison { error, reply })
    }
}

/// Handle to a governed multi-statement transaction running on its own task.
///
/// The task owns the PG transaction AND the semaphore permit, and self-
/// terminates on the first of: commit, rollback, the wall-time deadline, or this
/// handle being dropped (worker crash/stop closes the command channel). On exit
/// it reports its `tx_id` on the `done` channel so the supervisor can clear its
/// slot. The task and TX permit are released by `TX_MAX_WALL_TIME`; at an
/// exhausted deadline the SQLx transaction is dropped so its driver-managed
/// rollback begins without awaiting more network I/O in this task.
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
        Self::begin_with_invocation(
            pool,
            app_schema,
            state,
            &InvocationContext::default(),
            done,
        )
        .await
    }

    pub async fn begin_with_invocation(
        pool: &PgPool,
        app_schema: &str,
        state: &ContextState,
        invocation: &InvocationContext,
        done: mpsc::Sender<String>,
    ) -> Result<Self, String> {
        // Fail fast when all TX slots are taken — never queue (would hold the
        // caller hostage) and never exceed the pool budget. The permit is moved
        // into the task and released only when the task exits.
        let permit = TX_SEMAPHORE
            .try_acquire()
            .map_err(|_| "too many concurrent transactions; retry".to_string())?;

        let deadline = TokioInstant::now() + TX_MAX_WALL_TIME;
        let pool = pool.clone();
        let app_schema = app_schema.to_string();
        let state = state.clone();
        let invocation = invocation.clone();
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
                let mut pg_tx = match tokio::time::timeout_at(
                    deadline,
                    begin_app_tx_with_invocation(
                        &pool, &app_schema, &state, &invocation, state.audit_actor_id,
                        state.audit_delegator_id, "app_tx", TIMEOUT_INTERACTIVE_MS,
                    ),
                ).await {
                    Ok(Ok(t)) => { let _ = ready_tx.send(Ok(())); t }
                    Ok(Err(e)) => { let _ = ready_tx.send(Err(e.to_string())); return; }
                    Err(_) => { let _ = ready_tx.send(Err("transaction deadline exceeded".into())); return; }
                };

                let mut failed: Option<String> = None;
                loop {
                    tokio::select! {
                        biased;
                        _ = sleep_until(deadline) => {
                            // The deadline is already exhausted, so do not await
                            // network I/O beyond it. Dropping Transaction starts
                            // SQLx's rollback-on-drop and releases our ownership.
                            drop(pg_tx);
                            break;
                        }
                        cmd = cmd_rx.recv() => match cmd {
                            Some(TxCmd::Exec { sql, params, reply }) => {
                                if let Some(reason) = &failed {
                                    let _ = reply.send(Err(format!("transaction is aborted: {reason}")));
                                    continue;
                                }
                                let execution = async {
                                    validate_sql(&sql)?;
                                    let args = build_typed_args(&mut *pg_tx, &sql, &params).await?;
                                    let rows = sqlx::query_with(&sql, args)
                                        .fetch_all(&mut *pg_tx).await
                                        .map_err(|e| e.to_string())?;
                                    serialize_rows(rows)
                                };
                                match tokio::time::timeout_at(deadline, execution).await {
                                    Ok(result) => {
                                        if let Err(error) = &result {
                                            failed = Some(error.clone());
                                        }
                                        let _ = reply.send(result);
                                    }
                                    Err(_) => {
                                        let _ = reply.send(Err("transaction deadline exceeded".into()));
                                        drop(pg_tx);
                                        break;
                                    }
                                }
                            }
                            Some(TxCmd::Poison { error, reply }) => {
                                if failed.is_none() {
                                    failed = Some(error);
                                }
                                let _ = reply.send(Ok(()));
                            }
                            Some(TxCmd::Commit { reply }) => {
                                if let Some(reason) = failed {
                                    let rollback = match tokio::time::timeout_at(deadline, pg_tx.rollback()).await {
                                        Ok(result) => result.map_err(|e| e.to_string()),
                                        Err(_) => Err("transaction deadline exceeded during rollback".into()),
                                    };
                                    let error = match rollback {
                                        Ok(()) => format!("transaction rolled back after statement error: {reason}"),
                                        Err(rollback_error) => format!(
                                            "transaction failed ({reason}) and rollback failed ({rollback_error})"
                                        ),
                                    };
                                    let _ = reply.send(Err(error));
                                } else {
                                    let result = match tokio::time::timeout_at(deadline, pg_tx.commit()).await {
                                        Ok(result) => result.map_err(|e| e.to_string()),
                                        Err(_) => Err("transaction deadline exceeded during commit".into()),
                                    };
                                    let _ = reply.send(result);
                                }
                                break;
                            }
                            Some(TxCmd::Rollback { reply }) => {
                                let result = match tokio::time::timeout_at(deadline, pg_tx.rollback()).await {
                                    Ok(result) => result.map_err(|e| e.to_string()),
                                    Err(_) => Err("transaction deadline exceeded during rollback".into()),
                                };
                                let _ = reply.send(result);
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
    fn rejects_privileged_statements_hidden_behind_comments() {
        // Comments precede the keyword, so a prefix blocklist never saw these.
        for bad in [
            "/*x*/RESET ROLE",
            "/*x*/SET rootcx.user_id='x'",
            "--\nRESET ROLE",
            "-- harmless\n\tSET ROLE core_super",
            "/**/SET ROLE core_super",
            "/*a/*b*/c*/RESET ROLE",
            "/*a*/ /*b*/ -- c\n RESET ALL",
            "  /* leading */ COPY t FROM '/etc/passwd'",
            // PostgreSQL ends a line comment on CR as well as LF. Scanning only
            // for LF would swallow the privileged statement into the comment and
            // classify the *next* line, accepting these as `SELECT 1`. The block
            // comment carrying the LF is what makes them ONE statement to the
            // server, so the extended protocol's refusal of multi-statement does
            // not cover this — the classifier is the only guard. Verified against
            // PostgreSQL 16: the first payload executes `SET ROLE` and the session
            // returns to the pool's superuser login role.
            "--\rRESET ROLE\nSELECT 1",
            "--\rSET ROLE core_super /*\n SELECT 1 */",
            "--\rSET rootcx.user_id = /*\n SELECT */ 'victim-uuid'",
            "-- x\rRESET ROLE /*\n SELECT 1 */",
            "--\r\nRESET ROLE",
        ] {
            assert!(validate_sql(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn rejects_unterminated_block_comment_without_hanging() {
        for bad in ["/* SELECT 1", "/*", "/*a/*b*/ SELECT 1", "/*/"] {
            assert!(validate_sql(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn rejects_statements_outside_the_allowlist() {
        // EXPLAIN stays out: EXPLAIN (ANALYZE) leaks exact counts of RLS-hidden
        // rows via "Rows Removed by Filter".
        for bad in [
            "EXPLAIN SELECT * FROM contacts",
            "EXPLAIN (ANALYZE) SELECT * FROM contacts",
            "MERGE INTO t USING s ON true",
            "CALL some_proc()",
            "PREPARE p AS SELECT 1",
            "BEGIN",
            "COMMIT",
            "LOCK TABLE contacts",
            "ANALYZE contacts",
            "SHOW ALL",
            "LISTEN c",
            "",
            "   ",
        ] {
            assert!(validate_sql(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn rejects_ddl_prefixes() {
        // Multi-statement is NOT checked here — sqlx's extended protocol blocks
        // it structurally.
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
            "SELECT * FROM offset_table",               // body word starting with "set"-like text
            "UPDATE t SET x = 1",                       // "SET" in the body, not the head
            "select 1",                                 // lowercase
            "\n\t  SELECT 1",                           // leading whitespace
            "/* app: crm */ SELECT * FROM contacts",     // leading block comment
            "-- daily report\nSELECT count(*) FROM t",   // leading line comment
            "/*a/*b*/c*/ SELECT 1",                     // nested leading comment
            "(SELECT 1) UNION (SELECT 2)",              // parenthesised SELECT
            "  ( SELECT 1 )",
            "WITH c AS (UPDATE t SET x = 1 RETURNING *) SELECT * FROM c",
            "VALUES (1), (2)",
            "TABLE contacts",
            "SELECT(1)",                                // no space after keyword
        ] {
            assert!(validate_sql(ok).is_ok(), "should allow: {ok}");
        }
    }

    #[test]
    fn numeric_parameters_reject_inexact_or_malformed_json_values() {
        for (value, expected) in [
            (serde_json::json!(12.34), "expected string"),
            (serde_json::json!(true), "expected string"),
            (serde_json::json!("not-a-decimal"), "invalid decimal"),
            (serde_json::json!(""), "invalid decimal"),
        ] {
            let mut args = sqlx::postgres::PgArguments::default();
            let error = bind_typed_value(&mut args, &value, "NUMERIC")
                .expect_err("NUMERIC parameters must preserve an exact decimal string");
            assert!(error.contains(expected), "value={value}: {error}");
        }
    }
}
