//! Real deploy/DB boundaries: app SQL is never executed or silently recorded.

mod harness;

use std::fs;
use std::path::Path;

use rootcx_core::app_migrations;
use serde_json::json;

fn write_migration(app_dir: &Path, name: &str, sql: &str) {
    let dir = app_dir.join("migrations");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), sql).unwrap();
}

#[tokio::test]
async fn pending_sql_is_refused_before_any_statement_or_bookkeeping() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("crm", "contacts").await;
    let app_dir = rt.runtime.data_dir().join("apps/crm");
    // The first file would succeed under the former owner runner; refusing only
    // dangerous-looking statements or failing partway through is insufficient.
    write_migration(
        &app_dir,
        "001_seed.sql",
        "INSERT INTO crm.contacts (first_name, last_name) VALUES ('injected', 'owner');",
    );
    write_migration(
        &app_dir,
        "002_owner.sql",
        "ALTER ROLE rootcx_app_executor BYPASSRLS;",
    );
    write_migration(&app_dir, "003_upper.SQL", "SELECT 1;");
    for _ in 0..2 {
        let error = app_migrations::run(rt.pool(), "crm", &app_dir)
            .await
            .unwrap_err();
        for expected in [
            "001_seed.sql",
            "002_owner.sql",
            "003_upper.SQL",
            "declarative manifest",
            "administrator",
            "No SQL was executed",
        ] {
            assert!(error.contains(expected), "missing {expected}: {error}");
        }
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM crm.contacts")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "not even the first pending statement may execute");
    let ledger: bool =
        sqlx::query_scalar("SELECT to_regclass('crm.schema_migrations') IS NOT NULL")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert!(!ledger, "refusal must not create bookkeeping");
    let bypass: bool = sqlx::query_scalar(
        "SELECT rolbypassrls FROM pg_roles WHERE rolname = 'rootcx_app_executor'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert!(!bypass, "migration SQL must never acquire owner authority");
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_accepts_no_pending_files_and_preserves_legacy_history() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("crm", "contacts").await;
    let backend = b"serve({rpc: {ping: () => 'ready'}});";
    sqlx::raw_sql(
        "CREATE TABLE crm.schema_migrations (filename text PRIMARY KEY);
        INSERT INTO crm.schema_migrations VALUES ('001_old.sql');",
    )
    .execute(rt.pool())
    .await
    .unwrap();
    for extra in [
        None,
        Some(("migrations/README.txt", b"not SQL".as_slice())),
        Some((
            "migrations/001_old.sql",
            b"THIS MUST NEVER BE PARSED OR EXECUTED".as_slice(),
        )),
    ] {
        let mut files = vec![("index.ts", backend.as_slice())];
        files.extend(extra);
        let (status, body) = rt.deploy("crm", &harness::make_tar_gz(&files)).await;
        assert_eq!(status, 200, "no pending files: {body}");
        assert_eq!(body["migrationsApplied"], json!([]));
        let (status, body) = rt
            .post_json("/api/v1/apps/crm/rpc", &json!({"method": "ping"}))
            .await;
        assert_eq!(status, 200, "deployed backend must run: {body}");
    }
    let app_dir = rt.runtime.data_dir().join("apps/crm");
    write_migration(
        &app_dir,
        "002_pending.sql",
        "DELETE FROM crm.schema_migrations;",
    );
    let error = app_migrations::run(rt.pool(), "crm", &app_dir)
        .await
        .unwrap_err();
    assert!(error.contains("002_pending.sql"), "{error}");
    assert!(
        !error.contains("001_old.sql"),
        "only pending files should be reported: {error}"
    );
    let history: Vec<String> =
        sqlx::query_scalar("SELECT filename FROM crm.schema_migrations ORDER BY filename")
            .fetch_all(rt.pool())
            .await
            .unwrap();
    assert_eq!(
        history,
        ["001_old.sql"],
        "neither erase history nor mark pending files applied"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_refuses_pending_sql_before_install_scripts_and_worker_start() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("crm", "contacts").await;
    let archive = harness::make_tar_gz(&[
        (
            "index.ts",
            b"serve({rpc: {ping: () => 'should not start'}});",
        ),
        (
            "package.json",
            br#"{"scripts":{"preinstall":"touch install-ran"}}"#,
        ),
        (
            "migrations/001_audit.sql",
            b"CREATE TABLE crm.audit_trail (note text);",
        ),
    ]);
    let (status, body) = rt.deploy("crm", &archive).await;
    assert_eq!(status, 400, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains("001_audit.sql") && error.contains("declarative manifest"),
        "{body}"
    );
    assert!(
        !rt.runtime.data_dir().join("apps/crm/install-ran").exists(),
        "pending SQL must be checked before install scripts"
    );
    assert!(
        rt.runtime
            .worker_manager()
            .worker_status("crm")
            .await
            .is_err(),
        "refused deploy must not spawn a worker"
    );
    // Failed archives remain on disk: the worker manager must enforce the same
    // refusal on manual/lazy starts and after a Core restart.
    let error = rt
        .runtime
        .worker_manager()
        .start_app(rt.pool(), rt.runtime.secret_manager(), "crm")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("001_audit.sql"),
        "manual start must also refuse: {error}"
    );
    let table: bool = sqlx::query_scalar("SELECT to_regclass('crm.audit_trail') IS NOT NULL")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert!(!table, "app SQL cannot execute through backend upload");
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_installs_dependencies_without_running_package_scripts() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("script_probe", "records").await;
    let app_dir = rt.runtime.data_dir().join("apps/script_probe");
    let root_marker = app_dir.join("root-install-ran");
    let dependency_marker = app_dir.join("dependency-install-ran");
    let marker_script = |path: &Path| format!(
        ": > '{}'", path.to_string_lossy().replace('\'', "'\\''"),
    );
    let package = json!({
        "name": "script-probe", "version": "1.0.0",
        "scripts": {"preinstall": marker_script(&root_marker)},
        "dependencies": {"local-probe": "file:./dependency"},
        "trustedDependencies": ["local-probe"]
    }).to_string();
    let dependency = json!({
        "name": "local-probe", "version": "1.0.0",
        "main": "index.js",
        "scripts": {"postinstall": marker_script(&dependency_marker)}
    }).to_string();
    let archive = harness::make_tar_gz(&[
        ("index.ts", b"import { value } from 'local-probe'; serve({rpc: {ping: () => value}});"),
        ("package.json", package.as_bytes()),
        ("dependency/package.json", dependency.as_bytes()),
        ("dependency/index.js", b"export const value = 'ready';"),
    ]);
    let (status, body) = rt.deploy("script_probe", &archive).await;
    assert_eq!(status, 200, "{body}");
    assert!(!root_marker.exists(), "root package scripts must not run with Core authority");
    assert!(!dependency_marker.exists(), "trusted dependency scripts must not run with Core authority");
    let (status, body) = rt
        .post_json("/api/v1/apps/script_probe/rpc", &json!({"method": "ping"}))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "ready", "installed dependency must be usable: {body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn unreadable_migration_directory_or_history_fails_closed() {
    let rt = harness::TestRuntime::boot().await;
    rt.install("crm", "contacts").await;
    let app_dir = rt.runtime.data_dir().join("apps/crm");
    fs::create_dir_all(&app_dir).unwrap();
    fs::write(app_dir.join("migrations"), "not a directory").unwrap();
    assert!(
        app_migrations::run(rt.pool(), "crm", &app_dir)
            .await
            .is_err(),
        "invalid migration directory must not be treated as absent"
    );
    fs::remove_file(app_dir.join("migrations")).unwrap();
    write_migration(&app_dir, "001_pending.sql", "SELECT 1;");
    sqlx::query("CREATE TABLE crm.schema_migrations (wrong_column text)")
        .execute(rt.pool())
        .await
        .unwrap();
    let error = app_migrations::run(rt.pool(), "crm", &app_dir)
        .await
        .unwrap_err();
    assert!(
        error.contains("read migration history"),
        "unreadable history cannot mean already applied: {error}"
    );
    rt.shutdown().await;
}
