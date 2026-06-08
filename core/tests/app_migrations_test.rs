//! Deploy-time app migration runner contract.
//!
//! Drives `app_migrations::run` against a real schema (the harness installs the
//! `crm.contacts` table), asserting the decisions the deploy path turns on:
//! apply-and-track, idempotent skip of already-applied files, and that a failing
//! file aborts WITHOUT recording itself while leaving prior files applied.

mod harness;

use std::fs;
use std::path::Path;

use rootcx_core::app_migrations;

fn write_migration(app_dir: &Path, name: &str, sql: &str) {
    let dir = app_dir.join("migrations");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), sql).unwrap();
}

async fn applied_files(rt: &harness::TestRuntime) -> Vec<String> {
    sqlx::query_scalar("SELECT filename FROM crm.schema_migrations ORDER BY filename")
        .fetch_all(rt.pool()).await.unwrap()
}

async fn index_exists(rt: &harness::TestRuntime, name: &str) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_indexes WHERE schemaname='crm' AND indexname=$1)")
        .bind(name).fetch_one(rt.pool()).await.unwrap()
}

#[tokio::test]
async fn applies_tracks_skips_and_aborts_on_failure() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("crm", "contacts").await;
    let app_dir = rt.runtime.data_dir().join("apps").join("crm");

    // ── First run: a single migration applies, is tracked, and took effect. ──
    write_migration(&app_dir, "001_idx.sql",
        "CREATE INDEX IF NOT EXISTS mtest_email_idx ON crm.contacts (email);");
    let applied = app_migrations::run(rt.pool(), "crm", &app_dir).await.unwrap();
    assert_eq!(applied, vec!["001_idx.sql"], "first run applies the new file");
    assert!(index_exists(&rt, "mtest_email_idx").await, "migration DDL took effect");
    assert_eq!(applied_files(&rt).await, vec!["001_idx.sql"], "applied file is recorded");

    // ── Second run: nothing pending, already-applied file is skipped. ──
    let again = app_migrations::run(rt.pool(), "crm", &app_dir).await.unwrap();
    assert!(again.is_empty(), "already-applied file is not re-run");

    // ── Failure: a broken file errors, is NOT recorded, prior file survives. ──
    write_migration(&app_dir, "002_bad.sql", "CREATE INDEX nonsense ON crm.does_not_exist (x);");
    let err = app_migrations::run(rt.pool(), "crm", &app_dir).await.unwrap_err();
    assert!(err.contains("002_bad.sql"), "error names the offending file: {err}");
    assert_eq!(applied_files(&rt).await, vec!["001_idx.sql"],
        "failed file leaves no record; the good one stays applied");

    rt.shutdown().await;
}
