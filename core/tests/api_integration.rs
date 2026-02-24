mod harness;
use harness::TestRuntime;
use rootcx_shared_types::SchemaVerification;
use serde_json::{Value, json};

fn make_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    make_tar_gz_raw(files, false)
}

fn make_tar_gz_raw(files: &[(&str, &[u8])], allow_unsafe: bool) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(enc);
    for &(name, data) in files {
        let mut header = tar::Header::new_gnu();
        if allow_unsafe {
            // Write path bytes directly to bypass tar crate's path validation
            let name_bytes = name.as_bytes();
            header.as_gnu_mut().unwrap().name[..name_bytes.len()].copy_from_slice(name_bytes);
        } else {
            header.set_path(name).unwrap();
        }
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

#[tokio::test]
async fn worker_does_not_receive_db_url() {
    let src = include_str!("../src/worker.rs");
    assert!(!src.contains("ROOTCX_DB_URL"), "worker.rs must not pass ROOTCX_DB_URL to worker processes");
}

#[tokio::test]
async fn sdk_does_not_persist_access_token_to_localstorage() {
    let auth_hook = include_str!("../../runtime/sdk/src/hooks/useAuth.ts");
    assert!(
        !auth_hook.contains("localStorage.setItem(TOKEN_KEY"),
        "useAuth must not persist access tokens to localStorage"
    );
    let provider = include_str!("../../runtime/sdk/src/components/RuntimeProvider.tsx");
    assert!(
        !provider.contains("localStorage.getItem(TOKEN_KEY)"),
        "RuntimeProvider must not restore access tokens from localStorage"
    );
}

#[tokio::test]
async fn mgmt_endpoints_reject_unauthenticated() {
    let rt = TestRuntime::boot_with_auth().await;
    let manifest = json!({
        "appId": "authtest", "name": "authtest", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "label", "type": "text", "required": true }
        ]}]
    });

    assert_eq!(rt.post_json_unauthed("/api/v1/apps", &manifest).await, 401);
    assert_eq!(rt.delete_unauthed("/api/v1/apps/authtest").await, 401);
    assert_eq!(rt.post_json_unauthed("/api/v1/apps/authtest/secrets", &json!({"key":"K","value":"V"})).await, 401);
    assert_eq!(rt.get_unauthed("/api/v1/apps/authtest/secrets").await, 401);
    assert_eq!(rt.delete_unauthed("/api/v1/apps/authtest/secrets/K").await, 401);
    assert_eq!(rt.post_json_unauthed("/api/v1/apps/authtest/jobs", &json!({"payload":{}})).await, 401);
    assert_eq!(rt.post_json_unauthed("/api/v1/apps/authtest/worker/start", &json!({})).await, 401);
    assert_eq!(rt.post_json_unauthed("/api/v1/apps/authtest/worker/stop", &json!({})).await, 401);
    rt.shutdown().await;
}

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
        .as_str()
        .unwrap()
        .to_string();

    let (s, updated) =
        rt.patch_json(&format!("/api/v1/apps/upd/collections/contacts/{id}"), &json!({"notes": "VIP"})).await;
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
        .as_str()
        .unwrap()
        .to_string();

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
                "unexpected failure reason: {:?}",
                job["error"]
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

#[tokio::test]
async fn rpc_missing_method() {
    let rt = TestRuntime::boot().await;
    rt.install("rpc", "items").await;
    let (s, _) = rt.post_json("/api/v1/apps/rpc/rpc", &json!({"params":{}})).await;
    assert_eq!(s, 400);
    rt.shutdown().await;
}

#[tokio::test]
async fn audit_trail() {
    let rt = TestRuntime::boot().await;
    rt.install("auditapp", "contacts").await;
    rt.create("auditapp", "contacts", &json!({"first_name":"A","last_name":"B"})).await;

    let (_, entries) = rt.get_json("/api/v1/audit?limit=20").await;
    let app: Vec<&Value> =
        entries.as_array().unwrap().iter().filter(|e| e["table_schema"].as_str() == Some("auditapp")).collect();
    assert!(!app.is_empty(), "audit log empty after INSERT: {entries:?}");
    assert_eq!(app[0]["operation"], "INSERT");
    rt.shutdown().await;
}

#[tokio::test]
async fn job_list_with_status_filter() {
    let rt = TestRuntime::boot().await;
    rt.install("jfilt", "items").await;

    rt.post_json("/api/v1/apps/jfilt/jobs", &json!({"payload":{"x":1}})).await;

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
    let (s, _) = rt
        .patch_json(
            "/api/v1/apps/updnf/collections/contacts/00000000-0000-0000-0000-000000000000",
            &json!({"notes": "nope"}),
        )
        .await;
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

#[tokio::test]
async fn audit_trail_update() {
    let rt = TestRuntime::boot().await;
    rt.install("audupd", "contacts").await;
    let created = rt.create("audupd", "contacts", &json!({"first_name":"A","last_name":"B"})).await;
    let id = created["id"].as_str().unwrap();

    rt.patch_json(&format!("/api/v1/apps/audupd/collections/contacts/{id}"), &json!({"notes":"updated"})).await;

    let (_, entries) = rt.get_json("/api/v1/audit?limit=50").await;
    let app: Vec<&Value> =
        entries.as_array().unwrap().iter().filter(|e| e["table_schema"].as_str() == Some("audupd")).collect();

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
    let del_entries: Vec<&Value> = entries
        .as_array()
        .unwrap()
        .iter()
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

#[tokio::test]
async fn deploy_rejects_empty_archive() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.deploy("dep-empty", &[]).await;
    assert_eq!(s, 400);
    assert!(body["error"].as_str().unwrap().contains("empty archive"));
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_rejects_corrupt_archive() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.deploy("dep-bad", b"not-a-tar-gz").await;
    assert_eq!(s, 500);
    assert!(body["error"].as_str().unwrap().contains("extract archive"));
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_without_entry_point_fails() {
    let rt = TestRuntime::boot().await;
    let archive = make_tar_gz(&[("README.md", b"# no entry point here")]);
    let (s, body) = rt.deploy("dep-noep", &archive).await;
    assert_eq!(s, 500);
    assert!(body["error"].as_str().unwrap().contains("no entry point"));
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_valid_archive() {
    let rt = TestRuntime::boot().await;
    let archive = make_tar_gz(&[("index.ts", b"process.stdin.resume();")]);
    let (s, body) = rt.deploy("dep-ok", &archive).await;
    assert_eq!(s, 200);
    assert!(body["message"].as_str().unwrap().contains("deployed and started"));
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_rejects_path_traversal() {
    let rt = TestRuntime::boot().await;
    let archive = make_tar_gz_raw(&[("../../../etc/evil.txt", b"pwned")], true);
    let (s, body) = rt.deploy("dep-traversal", &archive).await;
    assert!(s == 400, "expected 400, got {s}: {body}");
    assert!(body["error"].as_str().unwrap().contains("unsafe archive entry"));
    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_rejects_absolute_path() {
    let rt = TestRuntime::boot().await;
    let archive = make_tar_gz_raw(&[("/tmp/evil.txt", b"pwned")], true);
    let (s, body) = rt.deploy("dep-abs", &archive).await;
    assert!(s == 400, "expected 400, got {s}: {body}");
    assert!(body["error"].as_str().unwrap().contains("unsafe archive entry"));
    rt.shutdown().await;
}

#[tokio::test]
async fn rpc_on_unstarted_worker() {
    let rt = TestRuntime::boot().await;
    rt.install("rpcns", "items").await;
    let (s, body) = rt.post_json("/api/v1/apps/rpcns/rpc", &json!({"method":"ping"})).await;
    assert_eq!(s, 500);
    assert!(body["error"].as_str().unwrap().contains("no worker"));
    rt.shutdown().await;
}

#[tokio::test]
async fn install_empty_data_contract() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "emptydc", "name": "emptydc", "version": "1.0.0",
        "dataContract": []
    }))
    .await;

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
    }))
    .await;

    let (s1, _) = rt.post_json("/api/v1/apps/multi/collections/orders", &json!({"total": 42})).await;
    assert_eq!(s1, 201);
    let (s2, _) = rt.post_json("/api/v1/apps/multi/collections/items", &json!({"label": "widget"})).await;
    assert_eq!(s2, 201);

    let (_, orders) = rt.get_json("/api/v1/apps/multi/collections/orders").await;
    assert_eq!(orders.as_array().unwrap().len(), 1);
    let (_, items) = rt.get_json("/api/v1/apps/multi/collections/items").await;
    assert_eq!(items.as_array().unwrap().len(), 1);
    rt.shutdown().await;
}

fn typed_manifest() -> Value {
    json!({
        "appId": "typed", "name": "typed", "version": "1.0.0",
        "dataContract": [
            { "entityName": "parent", "fields": [
                { "name": "label", "type": "text", "required": true }
            ]},
            { "entityName": "child", "fields": [
                { "name": "ref_id", "type": "entity_link", "references": { "entity": "parent", "field": "id" } },
                { "name": "label", "type": "text" },
                { "name": "score", "type": "number" },
                { "name": "active", "type": "boolean" },
                { "name": "day", "type": "date" },
                { "name": "ts", "type": "timestamp" },
                { "name": "meta", "type": "json" },
                { "name": "tags", "type": "[text]" },
                { "name": "vals", "type": "[number]" },
            ]}
        ]
    })
}

#[tokio::test]
async fn typed_bindings_create_roundtrip() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&typed_manifest()).await;

    let parent = rt.create("typed", "parent", &json!({"label": "p"})).await;
    let pid = parent["id"].as_str().unwrap();

    let row = rt
        .create(
            "typed",
            "child",
            &json!({
                "ref_id": pid,
                "label": "x",
                "score": 42.5,
                "active": true,
                "day": "2026-03-15",
                "ts": "2026-03-15T10:30:00Z",
                "meta": {"k": 1},
                "tags": ["a", "b"],
                "vals": [1.1, 2.2],
            }),
        )
        .await;

    assert_eq!(row["ref_id"], pid);
    assert_eq!(row["label"], "x");
    assert_eq!(row["score"], 42.5);
    assert_eq!(row["active"], true);
    assert_eq!(row["day"], "2026-03-15");
    assert!(row["ts"].as_str().unwrap().starts_with("2026-03-15T10:30:00"), "ts={}", row["ts"]);
    assert_eq!(row["meta"], json!({"k": 1}));
    assert_eq!(row["tags"], json!(["a", "b"]));
    assert_eq!(row["vals"], json!([1.1, 2.2]));
    rt.shutdown().await;
}

#[tokio::test]
async fn typed_bindings_null_values() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&typed_manifest()).await;

    let row = rt
        .create(
            "typed",
            "child",
            &json!({
                "ref_id": null, "label": null, "score": null,
                "active": null, "day": null, "ts": null,
                "meta": null, "tags": null, "vals": null,
            }),
        )
        .await;

    for field in ["ref_id", "label", "score", "active", "day", "ts", "meta", "tags", "vals"] {
        assert!(row[field].is_null(), "{field} should be null, got: {}", row[field]);
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn typed_bindings_update() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&typed_manifest()).await;

    let id = rt.create("typed", "child", &json!({"label": "old"})).await["id"].as_str().unwrap().to_string();

    let (s, updated) = rt
        .patch_json(
            &format!("/api/v1/apps/typed/collections/child/{id}"),
            &json!({"day": "2026-12-25", "ts": "2026-12-25T00:00:00Z", "score": 99.9}),
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(updated["day"], "2026-12-25");
    assert!(updated["ts"].as_str().unwrap().starts_with("2026-12-25T00:00:00"), "ts={}", updated["ts"]);
    assert_eq!(updated["score"], 99.9);
    rt.shutdown().await;
}

#[tokio::test]
async fn typed_bindings_text_not_cast_as_date() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "txtdate", "name": "txtdate", "version": "1.0.0",
        "dataContract": [{ "entityName": "notes", "fields": [
            { "name": "body", "type": "text", "required": true },
        ]}]
    }))
    .await;

    for val in ["2026-01-01", "550e8400-e29b-41d4-a716-446655440000", "2026-01-01T00:00:00Z"] {
        let row = rt.create("txtdate", "notes", &json!({"body": val})).await;
        assert_eq!(row["body"], val, "text field should preserve literal string");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_adds_column() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "ssadd", "name": "ssadd", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
        ]}]
    }))
    .await;

    rt.create("ssadd", "items", &json!({"name": "first"})).await;

    rt.install_manifest(&json!({
        "appId": "ssadd", "name": "ssadd", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "email", "type": "text" },
        ]}]
    }))
    .await;

    let row = rt.create("ssadd", "items", &json!({"name": "second", "email": "a@b.com"})).await;
    assert_eq!(row["email"], "a@b.com");

    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_drops_column() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "ssdrop", "name": "ssdrop", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "legacy", "type": "text" },
        ]}]
    }))
    .await;

    rt.create("ssdrop", "items", &json!({"name": "one", "legacy": "old"})).await;

    rt.install_manifest(&json!({
        "appId": "ssdrop", "name": "ssdrop", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
        ]}]
    }))
    .await;

    let (s, rows) = rt.get_json("/api/v1/apps/ssdrop/collections/items").await;
    assert_eq!(s, 200);
    let arr = rows.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(arr[0].get("legacy").is_none(), "legacy column should be gone");

    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_changes_type() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "sstype", "name": "sstype", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "score", "type": "text" },
        ]}]
    }))
    .await;

    rt.create("sstype", "items", &json!({"name": "a", "score": "42"})).await;

    rt.install_manifest(&json!({
        "appId": "sstype", "name": "sstype", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "score", "type": "number" },
        ]}]
    }))
    .await;

    let row = rt.create("sstype", "items", &json!({"name": "b", "score": 99.5})).await;
    assert_eq!(row["score"], 99.5);

    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_changes_nullability() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "ssnull", "name": "ssnull", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "email", "type": "text" },
        ]}]
    }))
    .await;

    rt.create("ssnull", "items", &json!({"name": "a"})).await;

    let (_, rows) = rt.get_json("/api/v1/apps/ssnull/collections/items").await;
    let id = rows[0]["id"].as_str().unwrap();
    rt.delete(&format!("/api/v1/apps/ssnull/collections/items/{id}")).await;

    rt.install_manifest(&json!({
        "appId": "ssnull", "name": "ssnull", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "email", "type": "text", "required": true },
        ]}]
    }))
    .await;

    let (s, _) = rt.post_json("/api/v1/apps/ssnull/collections/items", &json!({"name": "b"})).await;
    assert_eq!(s, 500, "missing required field should fail");

    let (s, _) = rt.post_json("/api/v1/apps/ssnull/collections/items", &json!({"name": "c", "email": "c@d.com"})).await;
    assert_eq!(s, 201);

    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_changes_enum() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "ssenum", "name": "ssenum", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "status", "type": "text", "enum_values": ["draft", "active"] },
        ]}]
    }))
    .await;

    rt.create("ssenum", "items", &json!({"name": "a", "status": "draft"})).await;

    rt.install_manifest(&json!({
        "appId": "ssenum", "name": "ssenum", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "status", "type": "text", "enum_values": ["draft", "active", "archived"] },
        ]}]
    }))
    .await;

    let row = rt.create("ssenum", "items", &json!({"name": "b", "status": "archived"})).await;
    assert_eq!(row["status"], "archived");

    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_idempotent() {
    let rt = TestRuntime::boot().await;

    let manifest = json!({
        "appId": "ssidem", "name": "ssidem", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "score", "type": "number" },
        ]}]
    });

    rt.install_manifest(&manifest).await;
    rt.create("ssidem", "items", &json!({"name": "a", "score": 10})).await;

    rt.install_manifest(&manifest).await;

    let (s, rows) = rt.get_json("/api/v1/apps/ssidem/collections/items").await;
    assert_eq!(s, 200);
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["name"], "a");

    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_preserves_data() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "sspres", "name": "sspres", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "score", "type": "number" },
        ]}]
    }))
    .await;

    rt.create("sspres", "items", &json!({"name": "a", "score": 1})).await;
    rt.create("sspres", "items", &json!({"name": "b", "score": 2})).await;
    rt.create("sspres", "items", &json!({"name": "c", "score": 3})).await;

    rt.install_manifest(&json!({
        "appId": "sspres", "name": "sspres", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "score", "type": "number" },
            { "name": "email", "type": "text" },
        ]}]
    }))
    .await;

    let (s, rows) = rt.get_json("/api/v1/apps/sspres/collections/items").await;
    assert_eq!(s, 200);
    let arr = rows.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    for row in arr {
        assert!(row["email"].is_null(), "email should be null for old records");
    }

    rt.shutdown().await;
}

#[tokio::test]
async fn schema_sync_multiple_entities() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "ssmulti", "name": "ssmulti", "version": "1.0.0",
        "dataContract": [
            { "entityName": "contacts", "fields": [
                { "name": "name", "type": "text", "required": true },
                { "name": "phone", "type": "text" },
            ]},
            { "entityName": "notes", "fields": [
                { "name": "body", "type": "text", "required": true },
                { "name": "legacy", "type": "text" },
            ]},
        ]
    }))
    .await;

    rt.create("ssmulti", "contacts", &json!({"name": "alice", "phone": "123"})).await;
    rt.create("ssmulti", "notes", &json!({"body": "hello", "legacy": "x"})).await;

    rt.install_manifest(&json!({
        "appId": "ssmulti", "name": "ssmulti", "version": "1.1.0",
        "dataContract": [
            { "entityName": "contacts", "fields": [
                { "name": "name", "type": "text", "required": true },
                { "name": "phone", "type": "text" },
                { "name": "email", "type": "text" },
            ]},
            { "entityName": "notes", "fields": [
                { "name": "body", "type": "text", "required": true },
            ]},
        ]
    }))
    .await;

    let row = rt.create("ssmulti", "contacts", &json!({"name": "bob", "email": "b@c.com"})).await;
    assert_eq!(row["email"], "b@c.com");

    let (_, rows) = rt.get_json("/api/v1/apps/ssmulti/collections/notes").await;
    assert!(rows[0].get("legacy").is_none(), "legacy should be dropped from notes");

    rt.shutdown().await;
}

#[tokio::test]
async fn verify_schema_compliant() {
    let rt = TestRuntime::boot().await;
    let manifest = json!({
        "appId": "vsc", "name": "vsc", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
        ]}]
    });
    rt.install_manifest(&manifest).await;

    let (s, body) = rt.post_json("/api/v1/apps/schema/verify", &manifest).await;
    assert_eq!(s, 200);
    let result: SchemaVerification = serde_json::from_value(body).unwrap();
    assert!(result.compliant, "same manifest should be compliant");
    assert!(result.changes.is_empty());
    rt.shutdown().await;
}

#[tokio::test]
async fn verify_schema_detects_new_column() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "vsnc", "name": "vsnc", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
        ]}]
    }))
    .await;

    let v2 = json!({
        "appId": "vsnc", "name": "vsnc", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "email", "type": "text" },
        ]}]
    });
    let (s, body) = rt.post_json("/api/v1/apps/schema/verify", &v2).await;
    assert_eq!(s, 200);
    let result: SchemaVerification = serde_json::from_value(body).unwrap();
    assert!(!result.compliant);
    assert_eq!(result.changes.len(), 1);
    assert_eq!(result.changes[0].change_type, "add_column");
    assert_eq!(result.changes[0].column, "email");
    rt.shutdown().await;
}

#[tokio::test]
async fn verify_schema_detects_drop() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "vsd", "name": "vsd", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "legacy", "type": "text" },
        ]}]
    }))
    .await;

    let v2 = json!({
        "appId": "vsd", "name": "vsd", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
        ]}]
    });
    let (s, body) = rt.post_json("/api/v1/apps/schema/verify", &v2).await;
    assert_eq!(s, 200);
    let result: SchemaVerification = serde_json::from_value(body).unwrap();
    assert!(!result.compliant);
    assert!(result.changes.iter().any(|c| c.change_type == "drop_column" && c.column == "legacy"));
    rt.shutdown().await;
}

#[tokio::test]
async fn verify_schema_detects_type_change() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "vstc", "name": "vstc", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "score", "type": "text" },
        ]}]
    }))
    .await;

    let v2 = json!({
        "appId": "vstc", "name": "vstc", "version": "1.1.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "score", "type": "number" },
        ]}]
    });
    let (s, body) = rt.post_json("/api/v1/apps/schema/verify", &v2).await;
    assert_eq!(s, 200);
    let result: SchemaVerification = serde_json::from_value(body).unwrap();
    assert!(!result.compliant);
    assert!(result.changes.iter().any(|c| c.change_type == "alter_type" && c.column == "score"));
    rt.shutdown().await;
}

#[tokio::test]
async fn verify_schema_no_table_detects_create() {
    let rt = TestRuntime::boot().await;
    let manifest = json!({
        "appId": "vsnt", "name": "vsnt", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true },
        ]}]
    });
    let (s, body) = rt.post_json("/api/v1/apps/schema/verify", &manifest).await;
    assert_eq!(s, 200);
    let result: SchemaVerification = serde_json::from_value(body).unwrap();
    assert!(!result.compliant, "missing table should be detected as create_table change");
    assert!(result.changes.iter().any(|c| c.change_type == "create_table" && c.entity == "items"));
    rt.shutdown().await;
}
