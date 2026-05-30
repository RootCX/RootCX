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

const MAX_ROWS: usize = 10_000;

/// Resolved identity for a unit of work, looked up from a `context_token`.
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

/// 256-bit opaque handle. Not derivable from any message id.
pub fn new_context_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
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
/// the RLS identity GUCs, the audit attribution GUCs, then a drop to the
/// non-superuser executor role. Every set_config runs while still superuser
/// (the executor has it revoked). Callers run their statements on the returned
/// tx and commit.
pub async fn begin_app_tx<'a>(
    pool: &'a PgPool,
    app_schema: &str,
    state: &ContextState,
    audit_actor: Option<Uuid>,
    audit_delegator: Option<Uuid>,
    trigger_ref: &str,
) -> Result<sqlx::Transaction<'a, sqlx::Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(&format!("SET LOCAL search_path TO {}, public", quote_ident(app_schema)))
        .execute(&mut *tx).await?;
    set_rls_context(&mut tx, state).await?;
    crate::extensions::audit::set_context(&mut tx, audit_actor, audit_delegator, trigger_ref).await;
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
    &["CREATE", "DROP", "ALTER", "TRUNCATE", "GRANT", "REVOKE", "REINDEX", "VACUUM", "COPY", "SET ", "RESET", "DO "];

fn validate_sql(sql: &str) -> Result<(), String> {
    let head = sql.trim_start().to_ascii_uppercase();
    match BLOCKED_PREFIXES.iter().find(|kw| head.starts_with(*kw)) {
        Some(kw) => Err(format!("statement not allowed: {}", kw.trim())),
        None => Ok(()),
    }
}

pub struct SqlOk {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<JsonValue>>,
    pub row_count: usize,
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

    // rootcx_system is excluded from the search_path: the app cannot reach
    // system tables by unqualified name, and the role lacks SELECT on them.
    let mut tx = begin_app_tx(pool, app_schema, state, state.user_id, None, "app_sql")
        .await.map_err(|e| e.to_string())?;

    let mut q = sqlx::query(sql);
    for p in params {
        q = match p {
            JsonValue::Null => q.bind(Option::<String>::None),
            JsonValue::Bool(b) => q.bind(*b),
            JsonValue::Number(n) => match n.as_i64() {
                Some(i) => q.bind(i),
                None => q.bind(n.as_f64().unwrap_or(0.0)),
            },
            JsonValue::String(s) => q.bind(s.clone()),
            other => q.bind(other.to_string()),
        };
    }

    let rows = q.fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;

    if rows.is_empty() {
        return Ok(SqlOk { columns: vec![], rows: vec![], row_count: 0 });
    }
    // Never silently truncate: an over-large result is an explicit error so the
    // app paginates rather than processing a partial set as if complete.
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
            "SET ROLE rootcx_owner",
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
        ] {
            assert!(validate_sql(ok).is_ok(), "should allow: {ok}");
        }
    }

    #[test]
    fn token_is_256_bit_hex() {
        let t = new_context_token();
        assert_eq!(t.len(), 64);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(new_context_token(), new_context_token());
    }
}
