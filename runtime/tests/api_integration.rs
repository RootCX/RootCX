//! Integration tests for the RootCX Runtime API.
//! Replaces `test-e2e.sh`. Run: `cargo test --test api_integration`

mod harness;
use harness::TestRuntime;
use serde_json::{json, Value};

// ── Health & Status ─────────────────────────────────────────────

#[tokio::test]
async fn health_check() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.get_json("/health").await;
    assert_eq!(s, 200);
    assert_eq!(body["status"], "ok");
    rt.shutdown().await;
}

#[tokio::test]
async fn status_returns_online() {
    let rt = TestRuntime::boot().await;
    let (_, body) = rt.get_json("/api/v1/status").await;
    assert_eq!(body["runtime"]["state"], "online");
    assert_eq!(body["postgres"]["state"], "online");
    assert_eq!(body["postgres"]["port"], rt.pg_port);
    rt.shutdown().await;
}

// ── App Management ──────────────────────────────────────────────

#[tokio::test]
async fn install_and_list_apps() {
    let rt = TestRuntime::boot().await;
    rt.install("testapp", "contacts").await;

    let (_, body) = rt.get_json("/api/v1/apps").await;
    let apps = body.as_array().unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0]["id"], "testapp");
    assert_eq!(apps[0]["status"], "installed");
    rt.shutdown().await;
}

#[tokio::test]
async fn install_idempotent() {
    let rt = TestRuntime::boot().await;
    rt.install("idem", "items").await;
    rt.install("idem", "items").await;

    let (_, body) = rt.get_json("/api/v1/apps").await;
    assert_eq!(body.as_array().unwrap().len(), 1);
    rt.shutdown().await;
}

#[tokio::test]
async fn uninstall_app() {
    let rt = TestRuntime::boot().await;
    rt.install("todel", "things").await;
    assert_eq!(rt.delete("/api/v1/apps/todel").await, 200);

    let (_, body) = rt.get_json("/api/v1/apps").await;
    assert!(body.as_array().unwrap().is_empty());
    rt.shutdown().await;
}

// ── CRUD ────────────────────────────────────────────────────────

#[tokio::test]
async fn crud_create_list() {
    let rt = TestRuntime::boot().await;
    rt.install("crud", "contacts").await;

    for c in [
        json!({"first_name":"Alice","last_name":"Martin","email":"a@ex.com"}),
        json!({"first_name":"Bob","last_name":"Dupont","email":"b@ex.com"}),
        json!({"first_name":"Charlie","last_name":"Martin"}),
    ] {
        let v = rt.create("crud", "contacts", &c).await;
        assert!(v["id"].is_string());
    }

    let (_, body) = rt.get_json("/api/v1/apps/crud/collections/contacts").await;
    assert_eq!(body.as_array().unwrap().len(), 3);
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_get_by_id() {
    let rt = TestRuntime::boot().await;
    rt.install("getapp", "contacts").await;
    let created = rt.create("getapp", "contacts", &json!({"first_name":"Alice","last_name":"M"})).await;
    let id = created["id"].as_str().unwrap();

    let (s, fetched) = rt.get_json(&format!("/api/v1/apps/getapp/collections/contacts/{id}")).await;
    assert_eq!(s, 200);
    assert_eq!(fetched["first_name"], "Alice");
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_update() {
    let rt = TestRuntime::boot().await;
    rt.install("upd", "contacts").await;
    let id = rt.create("upd", "contacts", &json!({"first_name":"Alice","last_name":"M"})).await["id"]
        .as_str().unwrap().to_string();

    let (s, updated) = rt.patch_json(
        &format!("/api/v1/apps/upd/collections/contacts/{id}"),
        &json!({"notes": "VIP"}),
    ).await;
    assert_eq!(s, 200);
    assert_eq!(updated["notes"], "VIP");
    assert_eq!(updated["first_name"], "Alice");
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_delete() {
    let rt = TestRuntime::boot().await;
    rt.install("del", "contacts").await;
    let id = rt.create("del", "contacts", &json!({"first_name":"X","last_name":"Y"})).await["id"]
        .as_str().unwrap().to_string();

    assert_eq!(rt.delete(&format!("/api/v1/apps/del/collections/contacts/{id}")).await, 200);
    let (s, _) = rt.get_json(&format!("/api/v1/apps/del/collections/contacts/{id}")).await;
    assert_eq!(s, 404);
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_not_found() {
    let rt = TestRuntime::boot().await;
    rt.install("nf", "contacts").await;
    let (s, _) = rt.get_json("/api/v1/apps/nf/collections/contacts/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(s, 404);
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_invalid_uuid() {
    let rt = TestRuntime::boot().await;
    rt.install("uu", "contacts").await;
    let (s, body) = rt.get_json("/api/v1/apps/uu/collections/contacts/not-a-uuid").await;
    assert_eq!(s, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid UUID"));
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_empty_body() {
    let rt = TestRuntime::boot().await;
    rt.install("emp", "contacts").await;
    let (s, _) = rt.post_json("/api/v1/apps/emp/collections/contacts", &json!({})).await;
    assert_eq!(s, 400);
    rt.shutdown().await;
}

// ── Secrets ─────────────────────────────────────────────────────

#[tokio::test]
async fn secrets_crud() {
    let rt = TestRuntime::boot().await;
    rt.install("sec", "items").await;

    let (s, _) = rt.post_json("/api/v1/apps/sec/secrets", &json!({"key":"K","value":"v"})).await;
    assert_eq!(s, 200);

    let (_, keys) = rt.get_json("/api/v1/apps/sec/secrets").await;
    assert_eq!(keys, json!(["K"]));

    assert_eq!(rt.delete("/api/v1/apps/sec/secrets/K").await, 200);

    let (_, keys) = rt.get_json("/api/v1/apps/sec/secrets").await;
    assert_eq!(keys, json!([]));
    rt.shutdown().await;
}

#[tokio::test]
async fn secrets_delete_nonexistent() {
    let rt = TestRuntime::boot().await;
    rt.install("secnf", "items").await;
    assert_eq!(rt.delete("/api/v1/apps/secnf/secrets/NOPE").await, 404);
    rt.shutdown().await;
}

#[tokio::test]
async fn secrets_missing_fields() {
    let rt = TestRuntime::boot().await;
    rt.install("smf", "items").await;
    let (s1, _) = rt.post_json("/api/v1/apps/smf/secrets", &json!({"key":"K"})).await;
    let (s2, _) = rt.post_json("/api/v1/apps/smf/secrets", &json!({"value":"V"})).await;
    assert_eq!(s1, 400);
    assert_eq!(s2, 400);
    rt.shutdown().await;
}

// ── Jobs ────────────────────────────────────────────────────────

#[tokio::test]
async fn jobs_enqueue_and_get() {
    let rt = TestRuntime::boot().await;
    rt.install("job", "items").await;

    let (s, body) = rt.post_json("/api/v1/apps/job/jobs", &json!({"payload":{"task":"csv"}})).await;
    assert_eq!(s, 201);
    let job_id = body["job_id"].as_str().unwrap();

    let (_, job) = rt.get_json(&format!("/api/v1/apps/job/jobs/{job_id}")).await;
    assert_eq!(job["app_id"], "job");
    let status = job["status"].as_str().unwrap();
    match status {
        "pending" | "running" => {}
        "failed" => {
            assert!(
                job["error"].as_str().unwrap().contains("no worker"),
                "unexpected failure reason: {:?}", job["error"]
            );
        }
        other => panic!("unexpected job status: {other}"),
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn jobs_list() {
    let rt = TestRuntime::boot().await;
    rt.install("jl", "items").await;
    for i in 0..3 {
        rt.post_json("/api/v1/apps/jl/jobs", &json!({"payload":{"i":i}})).await;
    }

    let (_, body) = rt.get_json("/api/v1/apps/jl/jobs").await;
    assert_eq!(body.as_array().unwrap().len(), 3);
    rt.shutdown().await;
}

// ── File Upload ─────────────────────────────────────────────────

#[tokio::test]
async fn upload_file() {
    let rt = TestRuntime::boot().await;
    rt.install("up", "items").await;

    let csv = b"first_name,last_name\nDiana,Prince\n";
    let (s, body) = rt.upload("/api/v1/apps/up/upload", "import.csv", "text/csv", csv).await;
    assert_eq!(s, 201);
    assert!(body["file_id"].is_string());
    assert_eq!(body["original_name"], "import.csv");
    assert!(body["size"].as_u64().unwrap() > 0);
    rt.shutdown().await;
}

#[tokio::test]
async fn upload_rejects_bad_extension() {
    let rt = TestRuntime::boot().await;
    rt.install("upb", "items").await;

    let (s, body) = rt.upload("/api/v1/apps/upb/upload", "evil.sh", "text/plain", b"#!/bin/bash").await;
    assert_eq!(s, 400);
    assert!(body["error"].as_str().unwrap().contains("not allowed"));
    rt.shutdown().await;
}

// ── Workers ─────────────────────────────────────────────────────

#[tokio::test]
async fn worker_status_unstarted() {
    let rt = TestRuntime::boot().await;
    rt.install("ws", "items").await;
    let (s, body) = rt.get_json("/api/v1/apps/ws/worker/status").await;
    assert_eq!(s, 500);
    assert!(body["error"].as_str().unwrap().contains("no worker"));
    rt.shutdown().await;
}

#[tokio::test]
async fn all_worker_statuses() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.get_json("/api/v1/workers").await;
    assert_eq!(s, 200);
    assert!(body["workers"].is_object(), "expected workers to be an object, got: {body:?}");
    assert!(body["workers"].as_object().unwrap().is_empty());
    rt.shutdown().await;
}

// ── RPC ─────────────────────────────────────────────────────────

#[tokio::test]
async fn rpc_missing_method() {
    let rt = TestRuntime::boot().await;
    rt.install("rpc", "items").await;
    let (s, _) = rt.post_json("/api/v1/apps/rpc/rpc", &json!({"params":{}})).await;
    assert_eq!(s, 400);
    rt.shutdown().await;
}

// ── Audit ───────────────────────────────────────────────────────

#[tokio::test]
async fn audit_trail() {
    let rt = TestRuntime::boot().await;
    rt.install("auditapp", "contacts").await;
    rt.create("auditapp", "contacts", &json!({"first_name":"A","last_name":"B"})).await;

    let (_, entries) = rt.get_json("/api/v1/audit?limit=20").await;
    let app: Vec<&Value> = entries.as_array().unwrap().iter()
        .filter(|e| e["table_schema"].as_str() == Some("auditapp"))
        .collect();
    assert!(!app.is_empty(), "audit log empty after INSERT: {entries:?}");
    assert_eq!(app[0]["operation"], "INSERT");
    rt.shutdown().await;
}

// ── Jobs lifecycle (Phase 4) ────────────────────────────────────

#[tokio::test]
async fn job_list_with_status_filter() {
    let rt = TestRuntime::boot().await;
    rt.install("jfilt", "items").await;

    rt.post_json("/api/v1/apps/jfilt/jobs", &json!({"payload":{"x":1}})).await;

    // Poll until the job fails (no worker running) instead of a hard sleep
    for _ in 0..30 {
        let (s, body) = rt.get_json("/api/v1/apps/jfilt/jobs?status=failed").await;
        if s == 200 && body.as_array().map_or(false, |a| !a.is_empty()) {
            let jobs = body.as_array().unwrap();
            assert!(jobs.iter().all(|j| j["status"] == "failed"));
            rt.shutdown().await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("job did not reach 'failed' status within timeout");
}

#[tokio::test]
async fn job_not_found() {
    let rt = TestRuntime::boot().await;
    rt.install("jnf", "items").await;
    let (s, _) = rt.get_json("/api/v1/apps/jnf/jobs/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(s, 404);
    rt.shutdown().await;
}

#[tokio::test]
async fn job_invalid_uuid() {
    let rt = TestRuntime::boot().await;
    rt.install("juu", "items").await;
    let (s, body) = rt.get_json("/api/v1/apps/juu/jobs/not-a-uuid").await;
    assert_eq!(s, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid UUID"));
    rt.shutdown().await;
}

// ── CRUD edge cases (Phase 4) ──────────────────────────────────

#[tokio::test]
async fn crud_create_with_null_fields() {
    let rt = TestRuntime::boot().await;
    rt.install("cnull", "contacts").await;
    let created = rt.create("cnull", "contacts", &json!({"first_name":"A","last_name":"B","email":null})).await;
    let id = created["id"].as_str().unwrap();

    let (_, fetched) = rt.get_json(&format!("/api/v1/apps/cnull/collections/contacts/{id}")).await;
    assert_eq!(fetched["first_name"], "A");
    assert!(fetched["email"].is_null());
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_update_nonexistent() {
    let rt = TestRuntime::boot().await;
    rt.install("updnf", "contacts").await;
    let (s, _) = rt.patch_json(
        "/api/v1/apps/updnf/collections/contacts/00000000-0000-0000-0000-000000000000",
        &json!({"notes": "nope"}),
    ).await;
    assert_eq!(s, 404);
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_delete_nonexistent() {
    let rt = TestRuntime::boot().await;
    rt.install("delnf", "contacts").await;
    let s = rt.delete("/api/v1/apps/delnf/collections/contacts/00000000-0000-0000-0000-000000000000").await;
    assert_eq!(s, 404);
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_create_has_timestamps() {
    let rt = TestRuntime::boot().await;
    rt.install("ts", "contacts").await;
    let created = rt.create("ts", "contacts", &json!({"first_name":"T","last_name":"S"})).await;
    assert!(created["created_at"].is_string(), "missing created_at: {created:?}");
    assert!(created["updated_at"].is_string(), "missing updated_at: {created:?}");
    rt.shutdown().await;
}

// ── Secrets (Phase 4) ──────────────────────────────────────────

#[tokio::test]
async fn secrets_overwrite_existing() {
    let rt = TestRuntime::boot().await;
    rt.install("secow", "items").await;
    rt.post_json("/api/v1/apps/secow/secrets", &json!({"key":"K","value":"v1"})).await;
    rt.post_json("/api/v1/apps/secow/secrets", &json!({"key":"K","value":"v2"})).await;

    let (_, keys) = rt.get_json("/api/v1/apps/secow/secrets").await;
    assert_eq!(keys.as_array().unwrap().len(), 1);
    rt.shutdown().await;
}

// ── Audit lifecycle (Phase 4) ──────────────────────────────────

#[tokio::test]
async fn audit_trail_update() {
    let rt = TestRuntime::boot().await;
    rt.install("audupd", "contacts").await;
    let created = rt.create("audupd", "contacts", &json!({"first_name":"A","last_name":"B"})).await;
    let id = created["id"].as_str().unwrap();

    rt.patch_json(
        &format!("/api/v1/apps/audupd/collections/contacts/{id}"),
        &json!({"notes":"updated"}),
    ).await;

    let (_, entries) = rt.get_json("/api/v1/audit?limit=50").await;
    let app: Vec<&Value> = entries.as_array().unwrap().iter()
        .filter(|e| e["table_schema"].as_str() == Some("audupd"))
        .collect();

    let ops: Vec<&str> = app.iter().filter_map(|e| e["operation"].as_str()).collect();
    assert!(ops.contains(&"INSERT"), "missing INSERT in audit: {ops:?}");
    assert!(ops.contains(&"UPDATE"), "missing UPDATE in audit: {ops:?}");
    rt.shutdown().await;
}

#[tokio::test]
async fn audit_trail_delete() {
    let rt = TestRuntime::boot().await;
    rt.install("auddel", "contacts").await;
    let created = rt.create("auddel", "contacts", &json!({"first_name":"A","last_name":"B"})).await;
    let id = created["id"].as_str().unwrap();

    rt.delete(&format!("/api/v1/apps/auddel/collections/contacts/{id}")).await;

    let (_, entries) = rt.get_json("/api/v1/audit?limit=50").await;
    let del_entries: Vec<&Value> = entries.as_array().unwrap().iter()
        .filter(|e| e["table_schema"].as_str() == Some("auddel") && e["operation"].as_str() == Some("DELETE"))
        .collect();

    assert!(!del_entries.is_empty(), "no DELETE audit entry");
    assert!(del_entries[0]["old_record"].is_object(), "DELETE should have old_record");
    assert!(del_entries[0]["new_record"].is_null(), "DELETE new_record should be null");
    rt.shutdown().await;
}

#[tokio::test]
async fn audit_filters_by_app_id() {
    let rt = TestRuntime::boot().await;
    rt.install("audfa", "contacts").await;
    rt.install("audfb", "contacts").await;
    rt.create("audfa", "contacts", &json!({"first_name":"A","last_name":"A"})).await;
    rt.create("audfb", "contacts", &json!({"first_name":"B","last_name":"B"})).await;

    let (_, entries) = rt.get_json("/api/v1/audit?app_id=audfa&limit=50").await;
    let arr = entries.as_array().unwrap();
    assert!(!arr.is_empty());
    assert!(arr.iter().all(|e| e["table_schema"].as_str() == Some("audfa")));
    rt.shutdown().await;
}

// ── Workers/RPC (Phase 4) ─────────────────────────────────────

#[tokio::test]
async fn rpc_on_unstarted_worker() {
    let rt = TestRuntime::boot().await;
    rt.install("rpcns", "items").await;
    let (s, body) = rt.post_json("/api/v1/apps/rpcns/rpc", &json!({"method":"ping"})).await;
    assert_eq!(s, 500);
    assert!(body["error"].as_str().unwrap().contains("no worker"));
    rt.shutdown().await;
}

// ── App management (Phase 4) ───────────────────────────────────

#[tokio::test]
async fn install_empty_data_contract() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "emptydc", "name": "emptydc", "version": "1.0.0",
        "dataContract": []
    })).await;

    let (_, apps) = rt.get_json("/api/v1/apps").await;
    let app = apps.as_array().unwrap().iter().find(|a| a["id"] == "emptydc").unwrap();
    assert_eq!(app["status"], "installed");
    assert!(app["entities"].as_array().unwrap().is_empty());
    rt.shutdown().await;
}

#[tokio::test]
async fn uninstall_nonexistent_app() {
    let rt = TestRuntime::boot().await;
    let s = rt.delete("/api/v1/apps/doesnotexist").await;
    assert_eq!(s, 200, "uninstall of nonexistent app should be idempotent");
    rt.shutdown().await;
}

#[tokio::test]
async fn install_multiple_entities() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "multi", "name": "multi", "version": "1.0.0",
        "dataContract": [
            { "entityName": "orders", "fields": [
                { "name": "total", "type": "number", "required": true }
            ]},
            { "entityName": "items", "fields": [
                { "name": "label", "type": "text", "required": true }
            ]}
        ]
    })).await;

    // Create records in both entities independently
    let (s1, _) = rt.post_json("/api/v1/apps/multi/collections/orders", &json!({"total": 42})).await;
    assert_eq!(s1, 201);
    let (s2, _) = rt.post_json("/api/v1/apps/multi/collections/items", &json!({"label": "widget"})).await;
    assert_eq!(s2, 201);

    // List each entity independently
    let (_, orders) = rt.get_json("/api/v1/apps/multi/collections/orders").await;
    assert_eq!(orders.as_array().unwrap().len(), 1);
    let (_, items) = rt.get_json("/api/v1/apps/multi/collections/items").await;
    assert_eq!(items.as_array().unwrap().len(), 1);
    rt.shutdown().await;
}
