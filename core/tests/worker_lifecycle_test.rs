//! Lifecycle authority is enforced across the real Bun IPC/PostgreSQL boundary.

mod harness;

use std::time::Duration;

use serde_json::{Value, json};
use tokio::time::timeout;
use uuid::Uuid;

#[tokio::test]
async fn bun_lifecycle_and_anonymous_workers_cannot_read_or_fabricate_assignments() {
    let rt = harness::TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "lifecycle", "name": "Lifecycle probe", "version": "1.0.0",
        "dataContract": [
            {"entityName": "records", "fields": [{"name": "name", "type": "text"}]},
            {"entityName": "assignments", "fields": [
                {"name": "user_id", "type": "uuid", "owner": true},
                {"name": "record_id", "type": "uuid"}
            ]}
        ]
    }))
    .await;
    let record = rt
        .create("lifecycle", "records", &json!({"name": "private"}))
        .await;
    rt.register_and_login("lifecycle-attacker@test.local").await;
    let user: Uuid = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.users WHERE email = 'lifecycle-attacker@test.local'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    let assignments_before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rootcx_system.rbac_assignments")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    let backend = r#"
const user = __USER__;
const record = __RECORD__;
async function probe(ctx) {
  const results = {};
  const attempt = async (name, fn) => {
    try { results[name] = {value: await fn()}; }
    catch (error) { results[name] = {error: error.message}; }
  };
  await attempt('read', () => ctx.collection('records').find({}));
  await attempt('findOne', () => ctx.collection('records').findOne({id: record}));
  await attempt('sqlRead', async () => (await ctx.sql('SELECT * FROM lifecycle.records')).rows);
  await attempt('transactionRead', () => ctx.transaction(async tx =>
    (await tx.sql('SELECT * FROM lifecycle.records')).rows));
  await attempt('assignment', () => ctx.collection('assignments').insert({user_id: user, record_id: record}));
  await attempt('insert', () => ctx.collection('records').insert({name: 'fabricated'}));
  await attempt('update', () => ctx.collection('records').update({id: record, name: 'stolen'}));
  await attempt('delete', () => ctx.collection('records').delete(record));
  await attempt('rbac', () => ctx.sql(
    'INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)', [user, 'admin']));
  return results;
}
serve({
  onStart: async ctx => log.info('lifecycle-result:' + JSON.stringify(await probe(ctx))),
  rpc: {probe: (_params, _caller, ctx) => probe(ctx)},
});
"#.replace("__USER__", &json!(user).to_string())
        .replace("__RECORD__", &record["id"].to_string());
    let app_dir = rt.runtime.data_dir().join("apps/lifecycle");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::write(app_dir.join("index.ts"), backend).unwrap();

    // Block the first read until the lifecycle log subscription is attached;
    // no timing-based sleep or privileged result storage inside the worker.
    let mut gate = rt.pool().begin().await.unwrap();
    sqlx::query("LOCK TABLE lifecycle.records IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *gate)
        .await
        .unwrap();
    let wm = rt.runtime.worker_manager();
    let mut logs = wm.subscribe_logs("lifecycle").await.unwrap();
    gate.commit().await.unwrap();
    let lifecycle: Value = timeout(Duration::from_secs(20), async {
        loop {
            let entry = logs.recv().await.unwrap();
            if let Some(result) = entry.message.strip_prefix("lifecycle-result:") {
                break serde_json::from_str(result).unwrap();
            }
        }
    })
    .await
    .expect("real Bun onStart must complete and report every probe");
    let anonymous = wm
        .rpc(
            "lifecycle",
            Uuid::new_v4().to_string(),
            "probe".into(),
            json!({}),
            None,
        )
        .await
        .expect("real anonymous Bun worker must complete the probe");
    for (principal, result) in [("lifecycle", lifecycle), ("anonymous", anonymous)] {
        for operation in ["read", "sqlRead", "transactionRead"] {
            assert_eq!(
                result[operation]["value"],
                json!([]),
                "{principal} {operation}: {result}"
            );
        }
        assert_eq!(
            result["findOne"]["value"],
            Value::Null,
            "{principal}: {result}"
        );
        assert!(
            result["findOne"].get("error").is_none(),
            "{principal}: {result}"
        );
        for operation in ["assignment", "insert", "update", "delete", "rbac"] {
            assert!(
                result[operation]["error"].is_string(),
                "{principal} {operation} must deny: {result}"
            );
        }
    }
    let rows: Vec<String> = sqlx::query_scalar("SELECT name FROM lifecycle.records")
        .fetch_all(rt.pool())
        .await
        .unwrap();
    assert_eq!(rows, ["private"], "startup must not change existing data");
    let assignments: i64 = sqlx::query_scalar("SELECT count(*) FROM lifecycle.assignments")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    assert_eq!(
        assignments, 0,
        "a worker cannot fabricate an ownership assignment at startup"
    );
    let assignments_after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rootcx_system.rbac_assignments")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    assert_eq!(
        assignments_after, assignments_before,
        "lifecycle startup must not grant any admin/user roles"
    );
    rt.shutdown().await;
}
