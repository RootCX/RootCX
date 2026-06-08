//! Deploy-time SQL migrations for an app.
//!
//! Apps run under the restricted `rootcx_app_executor` role at runtime and
//! cannot issue DDL — structural setup the manifest can't express (indexes,
//! triggers, CHECK constraints, seed data) lives in `backend/migrations/*.sql`
//! and is applied HERE, by the core, as the DB owner, once per file, before the
//! worker starts. This is the deploy-time counterpart to the runtime SQL proxy:
//! the admin who deploys already has full DB authority, so running owner DDL at
//! deploy is no escalation; the runtime sandbox is untouched.
//!
//! Convention: the manifest's `dataContract` owns tables and columns; migrations
//! own everything else and run AFTER manifest schema-sync (which the install
//! step performs before backend upload), so the tables they target already
//! exist. Files apply in lexicographic filename order, each in its own
//! transaction with its bookkeeping row, so a failure leaves no partial record
//! and aborts the deploy (the worker is never started against a half-built
//! schema). A session advisory lock serializes concurrent deploys.

use std::collections::HashSet;
use std::path::Path;

use sqlx::{Connection as _, Executor as _, PgPool};

use crate::manifest::quote_ident;

/// Apply pending migrations from `<app_dir>/migrations`. Returns the filenames
/// newly applied this run. No migrations dir, or none pending, is a clean no-op.
pub async fn run(pool: &PgPool, schema: &str, app_dir: &Path) -> Result<Vec<String>, String> {
    let dir = app_dir.join("migrations");
    if !dir.is_dir() {
        return Ok(vec![]);
    }

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("read migrations dir: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Ok(vec![]);
    }

    let q = quote_ident(schema);
    let lock_key = format!("{schema}.schema_migrations");

    // One session-scoped advisory lock on a reserved connection serializes
    // concurrent deploys; all per-file transactions share this connection.
    let mut conn = pool.acquire().await.map_err(|e| e.to_string())?;
    sqlx::query("SELECT pg_advisory_lock(hashtextextended($1, 0))")
        .bind(&lock_key)
        .execute(&mut *conn)
        .await
        .map_err(|e| e.to_string())?;

    let result = apply(&mut conn, &q, &files).await;

    // Always release the session lock, even on the error path.
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
        .bind(&lock_key)
        .execute(&mut *conn)
        .await;
    result
}

async fn apply(
    conn: &mut sqlx::PgConnection,
    qschema: &str,
    files: &[std::path::PathBuf],
) -> Result<Vec<String>, String> {
    conn.execute(
        format!(
            "CREATE TABLE IF NOT EXISTS {qschema}.schema_migrations (
                filename text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())"
        )
        .as_str(),
    )
    .await
    .map_err(|e| format!("create schema_migrations: {e}"))?;

    let applied: HashSet<String> = sqlx::query_scalar(&format!(
        "SELECT filename FROM {qschema}.schema_migrations"
    ))
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| e.to_string())?
    .into_iter()
    .collect();

    let mut newly = Vec::new();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if applied.contains(&name) {
            continue;
        }
        let body = std::fs::read_to_string(path).map_err(|e| format!("{name}: read: {e}"))?;

        let mut tx = conn.begin().await.map_err(|e| format!("{name}: begin: {e}"))?;
        // Unqualified names resolve to the app schema; the file + its bookkeeping
        // row commit atomically, so a crash mid-file records nothing. A bare `&str`
        // runs via the simple-query protocol (multi-statement files OK).
        tx.execute(format!("SET LOCAL search_path TO {qschema}, public").as_str())
            .await
            .map_err(|e| format!("{name}: search_path: {e}"))?;
        tx.execute(body.as_str())
            .await
            .map_err(|e| format!("{name}: {e}"))?;
        sqlx::query(&format!(
            "INSERT INTO {qschema}.schema_migrations (filename) VALUES ($1)"
        ))
        .bind(&name)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("{name}: record: {e}"))?;
        tx.commit().await.map_err(|e| format!("{name}: commit: {e}"))?;
        newly.push(name);
    }
    Ok(newly)
}
