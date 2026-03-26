mod harness;
use harness::TestRuntime;
use rootcx_types::SchemaVerification;
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
    let rt = TestRuntime::boot().await;
    let manifest = json!({
        "appId": "authtest", "name": "authtest", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "label", "type": "text", "required": true }
        ]}]
    });

    assert_eq!(rt.post_unauthed("/api/v1/apps", &manifest).await.0, 401);
    assert_eq!(rt.delete_unauthed("/api/v1/apps/authtest").await, 401);
    assert_eq!(rt.post_unauthed("/api/v1/apps/authtest/secrets", &json!({"key":"K","value":"V"})).await.0, 401);
    assert_eq!(rt.get_unauthed("/api/v1/apps/authtest/secrets").await, 401);
    assert_eq!(rt.delete_unauthed("/api/v1/apps/authtest/secrets/K").await, 401);
    assert_eq!(rt.post_unauthed("/api/v1/apps/authtest/jobs", &json!({"payload":{}})).await.0, 401);
    assert_eq!(rt.post_unauthed("/api/v1/apps/authtest/worker/start", &json!({})).await.0, 401);
    assert_eq!(rt.post_unauthed("/api/v1/apps/authtest/worker/stop", &json!({})).await.0, 401);
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
async fn uninstall_cleans_db_and_filesystem() {
    let rt = TestRuntime::boot().await;
    rt.install("cleanup", "contacts").await;

    rt.create("cleanup", "contacts", &json!({"first_name":"A","last_name":"B"})).await;
    rt.post_json("/api/v1/apps/cleanup/secrets", &json!({"key":"SK","value":"sv"})).await;
    rt.post_json("/api/v1/apps/cleanup/jobs", &json!({"payload":{"x":1}})).await;

    let archive = make_tar_gz(&[("index.ts", b"process.stdin.resume();")]);
    let (s, _) = rt.deploy("cleanup", &archive).await;
    assert_eq!(s, 200);

    assert_eq!(rt.delete("/api/v1/apps/cleanup").await, 200);

    // secrets and jobs rows deleted (new DB cleanup)
    let (_, keys) = rt.get_json("/api/v1/apps/cleanup/secrets").await;
    assert!(keys.as_array().map_or(true, |a| a.is_empty()), "secrets should be gone: {keys}");
    let (_, jobs) = rt.get_json("/api/v1/apps/cleanup/jobs").await;
    assert!(jobs.as_array().map_or(true, |a| a.is_empty()), "jobs should be gone: {jobs}");

    // reinstall + deploy works (filesystem was cleaned)
    rt.install("cleanup", "contacts").await;
    let (s, _) = rt.deploy("cleanup", &make_tar_gz(&[("index.ts", b"process.stdin.resume();")])).await;
    assert_eq!(s, 200, "reinstall after full uninstall should succeed");

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

// ── Platform Secrets ──

#[tokio::test]
async fn platform_secrets_crud() {
    let rt = TestRuntime::boot().await;
    let (s, _) = rt.post_json("/api/v1/platform/secrets", &json!({"key":"MY_KEY","value":"secret"})).await;
    assert_eq!(s, 200);

    let (_, keys) = rt.get_json("/api/v1/platform/secrets").await;
    assert!(keys.as_array().unwrap().contains(&json!("MY_KEY")));

    assert_eq!(rt.delete("/api/v1/platform/secrets/MY_KEY").await, 200);

    let (_, keys) = rt.get_json("/api/v1/platform/secrets").await;
    assert!(!keys.as_array().unwrap().contains(&json!("MY_KEY")));
    rt.shutdown().await;
}

#[tokio::test]
async fn platform_secrets_env() {
    let rt = TestRuntime::boot().await;
    rt.post_json("/api/v1/platform/secrets", &json!({"key":"ENV_KEY","value":"env_val"})).await;

    let (s, body) = rt.get_json("/api/v1/platform/secrets/env").await;
    assert_eq!(s, 200);
    assert_eq!(body["ENV_KEY"], "env_val");
    rt.shutdown().await;
}

#[tokio::test]
async fn platform_secrets_delete_nonexistent() {
    let rt = TestRuntime::boot().await;
    assert_eq!(rt.delete("/api/v1/platform/secrets/NOPE").await, 404);
    rt.shutdown().await;
}

#[tokio::test]
async fn platform_secrets_invalid_key() {
    let rt = TestRuntime::boot().await;
    for key in ["", "bad key!", "no-dashes", "no.dots"] {
        let (s, _) = rt.post_json("/api/v1/platform/secrets", &json!({"key": key, "value":"v"})).await;
        assert_eq!(s, 400, "key {key:?} should be rejected");
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn platform_secrets_change_triggers_worker_restart() {
    let rt = TestRuntime::boot().await;

    // set a platform secret — response should include workers_restarted
    let (s, body) = rt.post_json("/api/v1/platform/secrets", &json!({"key":"LLM_KEY","value":"val"})).await;
    assert_eq!(s, 200);
    assert!(body.get("workers_restarted").is_some(), "set: missing workers_restarted: {body}");

    // delete a platform secret — response should also include workers_restarted
    let (s, body) = rt.delete_json("/api/v1/platform/secrets/LLM_KEY").await;
    assert_eq!(s, 200);
    assert!(body.get("workers_restarted").is_some(), "delete: missing workers_restarted: {body}");

    rt.shutdown().await;
}

// ── AI Config ──

#[tokio::test]
async fn ai_config_get_set() {
    let rt = TestRuntime::boot().await;

    let (s, _) = rt.get_json("/api/v1/config/ai").await;
    assert_eq!(s, 404, "should be 404 before any config is set");

    let config = json!({"provider":"Anthropic","model":"claude-sonnet-4-20250514"});
    let (s, _) = rt.put_json("/api/v1/config/ai", &config).await;
    assert!(s == 200 || s == 204);

    let (s, body) = rt.get_json("/api/v1/config/ai").await;
    assert_eq!(s, 200);
    assert_eq!(body["provider"], "Anthropic");
    assert_eq!(body["model"], "claude-sonnet-4-20250514");
    rt.shutdown().await;
}

// ── DB Introspection ──

#[tokio::test]
async fn db_list_schemas() {
    let rt = TestRuntime::boot().await;
    rt.install("dbschema", "items").await;

    let (s, body) = rt.get_json("/api/v1/db/schemas").await;
    assert_eq!(s, 200);
    let names: Vec<&str> = body.as_array().unwrap().iter().filter_map(|s| s["schema_name"].as_str()).collect();
    assert!(names.contains(&"rootcx_system"), "should contain rootcx_system");
    assert!(names.contains(&"dbschema"), "should contain app schema");
    rt.shutdown().await;
}

#[tokio::test]
async fn db_list_tables() {
    let rt = TestRuntime::boot().await;
    rt.install("dbtables", "contacts").await;

    let (s, body) = rt.get_json("/api/v1/db/schemas/dbtables/tables").await;
    assert_eq!(s, 200);
    let tables: Vec<&str> = body.as_array().unwrap().iter().filter_map(|t| t["table_name"].as_str()).collect();
    assert!(tables.contains(&"contacts"));
    assert!(!body[0]["columns"].as_array().unwrap().is_empty());
    rt.shutdown().await;
}

#[tokio::test]
async fn db_execute_query() {
    let rt = TestRuntime::boot().await;
    rt.install("dbq", "items").await;
    rt.create("dbq", "items", &json!({"first_name":"A","last_name":"B"})).await;

    let (s, body) = rt.post_json("/api/v1/db/query", &json!({"sql":"SELECT * FROM items","schema":"dbq"})).await;
    assert_eq!(s, 200);
    assert!(body["columns"].is_array());
    assert_eq!(body["row_count"], 1);
    assert_eq!(body["rows"].as_array().unwrap().len(), 1);
    rt.shutdown().await;
}

#[tokio::test]
async fn db_query_rejects_dml() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.post_json("/api/v1/db/query", &json!({"sql":"DROP TABLE foo"})).await;
    assert_eq!(s, 400);
    assert!(body["error"].as_str().unwrap().to_lowercase().contains("select"));
    rt.shutdown().await;
}

// ── Query Records ──

#[tokio::test]
async fn query_records_basic() {
    let rt = TestRuntime::boot().await;
    rt.install("qr", "contacts").await;
    rt.create("qr", "contacts", &json!({"first_name":"Alice","last_name":"A"})).await;
    rt.create("qr", "contacts", &json!({"first_name":"Bob","last_name":"B"})).await;
    rt.create("qr", "contacts", &json!({"first_name":"Charlie","last_name":"C"})).await;

    let (s, body) = rt.post_json("/api/v1/apps/qr/collections/contacts/query", &json!({
        "where": {"first_name": "Alice"},
        "limit": 10
    })).await;
    assert_eq!(s, 200);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["first_name"], "Alice");
    assert_eq!(body["total"], 1);
    rt.shutdown().await;
}

#[tokio::test]
async fn query_records_operators() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId": "qrop", "name": "qrop", "version": "1.0.0",
        "dataContract": [{ "entityName": "scores", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "value", "type": "number", "required": true },
        ]}]
    })).await;
    for (n, v) in [("low", 10), ("mid", 50), ("high", 90)] {
        rt.create("qrop", "scores", &json!({"name": n, "value": v})).await;
    }

    let (s, body) = rt.post_json("/api/v1/apps/qrop/collections/scores/query", &json!({
        "where": {"value": {"$gte": 50}},
        "orderBy": "value", "order": "asc"
    })).await;
    assert_eq!(s, 200);
    let data = body["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["name"], "mid");
    assert_eq!(data[1]["name"], "high");
    rt.shutdown().await;
}

#[tokio::test]
async fn query_records_or_combinator() {
    let rt = TestRuntime::boot().await;
    rt.install("qror", "contacts").await;
    rt.create("qror", "contacts", &json!({"first_name":"Alice","last_name":"A"})).await;
    rt.create("qror", "contacts", &json!({"first_name":"Bob","last_name":"B"})).await;
    rt.create("qror", "contacts", &json!({"first_name":"Charlie","last_name":"C"})).await;

    let (s, body) = rt.post_json("/api/v1/apps/qror/collections/contacts/query", &json!({
        "where": {"$or": [{"first_name":"Alice"}, {"first_name":"Charlie"}]}
    })).await;
    assert_eq!(s, 200);
    assert_eq!(body["total"], 2);
    rt.shutdown().await;
}

// ── Bulk Create ──

#[tokio::test]
async fn bulk_create_records() {
    let rt = TestRuntime::boot().await;
    rt.install("bulk", "contacts").await;

    let records: Vec<Value> = (0..5).map(|i| json!({"first_name": format!("U{i}"), "last_name": "L"})).collect();
    let (s, body) = rt.post_json("/api/v1/apps/bulk/collections/contacts/bulk", &json!(records)).await;
    assert_eq!(s, 201);
    assert_eq!(body.as_array().unwrap().len(), 5);

    let (_, all) = rt.get_json("/api/v1/apps/bulk/collections/contacts").await;
    assert_eq!(all.as_array().unwrap().len(), 5);
    rt.shutdown().await;
}

#[tokio::test]
async fn bulk_create_empty_array() {
    let rt = TestRuntime::boot().await;
    rt.install("bulke", "contacts").await;
    let (s, _) = rt.post_json("/api/v1/apps/bulke/collections/contacts/bulk", &json!([])).await;
    assert_eq!(s, 400);
    rt.shutdown().await;
}

// ── Auth Flow ──

#[tokio::test]
async fn auth_mode_always_required() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.get_json("/api/v1/auth/mode").await;
    assert_eq!(s, 200);
    assert_eq!(body["authRequired"], true);
    rt.shutdown().await;
}

#[tokio::test]
async fn auth_register_login_me_logout() {
    let rt = TestRuntime::boot().await;

    // register new user
    let (s, body) = rt.post_unauthed(
        "/api/v1/auth/register",
        &json!({"email":"newuser@test.local","password":"Str0ngPass!"}),
    ).await;
    assert_eq!(s, 201);
    assert!(body["user"]["id"].is_string());

    // login
    let (s, body) = rt.post_unauthed(
        "/api/v1/auth/login",
        &json!({"email":"newuser@test.local","password":"Str0ngPass!"}),
    ).await;
    assert_eq!(s, 200);
    let access = body["accessToken"].as_str().unwrap();
    let refresh = body["refreshToken"].as_str().unwrap().to_string();
    assert!(body["expiresIn"].as_i64().unwrap() > 0);

    // me
    let r = rt.client.get(rt.url("/api/v1/auth/me")).bearer_auth(access).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let me: Value = r.json().await.unwrap();
    assert_eq!(me["email"], "newuser@test.local");

    // refresh
    let r = rt.client.post(rt.url("/api/v1/auth/refresh")).json(&json!({"refreshToken": refresh})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let rb: Value = r.json().await.unwrap();
    assert!(rb["accessToken"].is_string());

    // logout
    let r = rt.client.post(rt.url("/api/v1/auth/logout")).json(&json!({"refreshToken": refresh})).send().await.unwrap();
    assert_eq!(r.status(), 200);

    rt.shutdown().await;
}

#[tokio::test]
async fn auth_login_wrong_password() {
    let rt = TestRuntime::boot().await;
    let (s, _) = rt.post_unauthed(
        "/api/v1/auth/login",
        &json!({"email":"admin@test.local","password":"wrongpassword"}),
    ).await;
    assert_eq!(s, 401);
    rt.shutdown().await;
}

#[tokio::test]
async fn auth_register_weak_password() {
    let rt = TestRuntime::boot().await;
    let (s, _) = rt.post_unauthed(
        "/api/v1/auth/register",
        &json!({"email":"weak@test.local","password":"short"}),
    ).await;
    assert_eq!(s, 400);
    rt.shutdown().await;
}

#[tokio::test]
async fn auth_list_users() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.get_json("/api/v1/users").await;
    assert_eq!(s, 200);
    let users = body.as_array().unwrap();
    assert!(users.iter().any(|u| u["email"] == "admin@test.local"));
    rt.shutdown().await;
}

// ── RBAC ──

#[tokio::test]
async fn rbac_list_roles() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacapp", "items").await;

    let (s, body) = rt.get_json("/api/v1/apps/rbacapp/roles").await;
    assert_eq!(s, 200);
    let roles = body.as_array().unwrap();
    assert!(roles.iter().any(|r| r["name"] == "admin"), "should have built-in admin role");
    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_create_and_delete_role() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacc", "items").await;

    let (s, _) = rt.post_json("/api/v1/apps/rbacc/roles", &json!({
        "name": "editor",
        "description": "Can edit items",
        "permissions": ["items.read", "items.update"]
    })).await;
    assert_eq!(s, 200);

    let (_, roles) = rt.get_json("/api/v1/apps/rbacc/roles").await;
    assert!(roles.as_array().unwrap().iter().any(|r| r["name"] == "editor"));

    assert_eq!(rt.delete("/api/v1/apps/rbacc/roles/editor").await, 200);

    let (_, roles) = rt.get_json("/api/v1/apps/rbacc/roles").await;
    assert!(!roles.as_array().unwrap().iter().any(|r| r["name"] == "editor"));
    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_cannot_create_admin_role() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacadm", "items").await;

    let (s, _) = rt.post_json("/api/v1/apps/rbacadm/roles", &json!({"name":"admin"})).await;
    assert!(s == 400 || s == 409, "creating admin role should fail, got {s}");
    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_assign_and_revoke_role() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacas", "items").await;

    // register a second user
    rt.post_unauthed(
        "/api/v1/auth/register",
        &json!({"email":"rbacuser@test.local","password":"Str0ngPass1"}),
    ).await;

    // get user id
    let (_, users) = rt.get_json("/api/v1/users").await;
    let user_id = users.as_array().unwrap().iter()
        .find(|u| u["email"] == "rbacuser@test.local").unwrap()["id"].as_str().unwrap().to_string();

    // create role
    rt.post_json("/api/v1/apps/rbacas/roles", &json!({"name":"viewer","permissions":["items.read"]})).await;

    // assign
    let (s, _) = rt.post_json("/api/v1/apps/rbacas/roles/assign", &json!({"userId": user_id, "role": "viewer"})).await;
    assert_eq!(s, 200);

    // list assignments
    let (s, body) = rt.get_json("/api/v1/apps/rbacas/roles/assignments").await;
    assert_eq!(s, 200);
    assert!(body.as_array().unwrap().iter().any(|a| a["role"] == "viewer"));

    // revoke
    let (s, _) = rt.post_json("/api/v1/apps/rbacas/roles/revoke", &json!({"userId": user_id, "role": "viewer"})).await;
    assert_eq!(s, 200);

    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_my_permissions() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacperm", "items").await;

    let (s, body) = rt.get_json("/api/v1/apps/rbacperm/permissions").await;
    assert_eq!(s, 200);
    let roles: Vec<&str> = body["roles"].as_array().unwrap().iter().filter_map(|r| r.as_str()).collect();
    assert!(roles.contains(&"admin"), "first user should be admin");
    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_available_permissions() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacavail", "items").await;

    let (s, body) = rt.get_json("/api/v1/apps/rbacavail/permissions/available").await;
    assert_eq!(s, 200);
    let perms = body.as_array().unwrap();
    let keys: Vec<&str> = perms.iter().filter_map(|p| p["key"].as_str()).collect();
    assert!(keys.contains(&"items.read"));
    assert!(keys.contains(&"items.create"));
    rt.shutdown().await;
}

// ── MCP Servers ──

#[tokio::test]
async fn mcp_servers_register_and_remove() {
    let rt = TestRuntime::boot().await;

    let (s, body) = rt.post_json("/api/v1/mcp-servers", &json!({
        "name": "test-mcp",
        "transport": {"type": "stdio", "command": "echo", "args": ["hello"]}
    })).await;
    assert_eq!(s, 200);
    assert_eq!(body["name"], "test-mcp");

    let (s, body) = rt.get_json("/api/v1/mcp-servers/test-mcp").await;
    assert_eq!(s, 200);
    assert_eq!(body["name"], "test-mcp");

    let (_, list) = rt.get_json("/api/v1/mcp-servers").await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    assert_eq!(rt.delete("/api/v1/mcp-servers/test-mcp").await, 200);

    let (_, list) = rt.get_json("/api/v1/mcp-servers").await;
    assert!(list.as_array().unwrap().is_empty());
    rt.shutdown().await;
}

#[tokio::test]
async fn mcp_servers_get_nonexistent() {
    let rt = TestRuntime::boot().await;
    let (s, _) = rt.get_json("/api/v1/mcp-servers/nope").await;
    assert_eq!(s, 404);
    rt.shutdown().await;
}

// ── Tools Registry ──

#[tokio::test]
async fn tools_execute_unknown() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.post_json("/api/v1/tools/nonexistent/execute", &json!({"appId":"x","args":{}})).await;
    assert_eq!(s, 404);
    assert!(body["error"].as_str().unwrap().contains("unknown"));
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

#[tokio::test]
async fn identity_linked_query_cross_app() {
    let rt = TestRuntime::boot().await;

    // CRM with companies identified by name
    rt.install_manifest(&json!({
        "appId": "crm_link", "name": "CRM", "version": "1.0.0",
        "dataContract": [{ "entityName": "companies", "identityKind": "organization", "identityKey": "name", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "industry", "type": "text" }
        ]}]
    })).await;

    // Billing with customers identified by the same key
    rt.install_manifest(&json!({
        "appId": "bill_link", "name": "Billing", "version": "1.0.0",
        "dataContract": [{ "entityName": "customers", "identityKind": "organization", "identityKey": "name", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "vat_number", "type": "text" }
        ]}]
    })).await;

    // Create matching records
    rt.create("crm_link", "companies", &json!({"name": "Acme Corp", "industry": "Tech"})).await;
    rt.create("bill_link", "customers", &json!({"name": "Acme Corp", "vat_number": "BE123"})).await;
    rt.create("crm_link", "companies", &json!({"name": "Other Inc", "industry": "Finance"})).await;

    // Query with linked=true
    let (s, body) = rt.post_json("/api/v1/apps/crm_link/collections/companies/query", &json!({"linked": true})).await;
    assert_eq!(s, 200);
    let data = body["data"].as_array().unwrap();

    let acme = data.iter().find(|r| r["name"] == "Acme Corp").unwrap();
    assert!(acme["_linked"]["bill_link"].is_object(), "Acme should have billing linked data");
    assert_eq!(acme["_linked"]["bill_link"]["entity"], "customers");
    assert_eq!(acme["_linked"]["bill_link"]["data"]["vat_number"], "BE123");

    let other = data.iter().find(|r| r["name"] == "Other Inc").unwrap();
    assert!(other.get("_linked").is_none(), "Other Inc has no billing match");

    // Query with linked=["bill_link"] (subset)
    let (_, body2) = rt.post_json("/api/v1/apps/crm_link/collections/companies/query", &json!({"linked": ["bill_link"]})).await;
    let acme2 = body2["data"].as_array().unwrap().iter().find(|r| r["name"] == "Acme Corp").unwrap();
    assert!(acme2["_linked"]["bill_link"].is_object());

    // Query without linked — no _linked field
    let (_, body3) = rt.post_json("/api/v1/apps/crm_link/collections/companies/query", &json!({})).await;
    let acme3 = body3["data"].as_array().unwrap().iter().find(|r| r["name"] == "Acme Corp").unwrap();
    assert!(acme3.get("_linked").is_none(), "no _linked without opt-in");

    rt.shutdown().await;
}

#[tokio::test]
async fn identity_linked_via_get_query_param() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "crm_qp", "name": "CRM", "version": "1.0.0",
        "dataContract": [{ "entityName": "contacts", "identityKind": "person", "identityKey": "email", "fields": [
            { "name": "email", "type": "text", "required": true },
            { "name": "name", "type": "text" }
        ]}]
    })).await;
    rt.install_manifest(&json!({
        "appId": "help_qp", "name": "Helpdesk", "version": "1.0.0",
        "dataContract": [{ "entityName": "requesters", "identityKind": "person", "identityKey": "email", "fields": [
            { "name": "email", "type": "text", "required": true },
            { "name": "priority", "type": "text" }
        ]}],
        "permissions": { "permissions": [{ "key": "requesters.read", "description": "read" }]}
    })).await;

    rt.create("crm_qp", "contacts", &json!({"email": "john@acme.com", "name": "John"})).await;
    rt.create("help_qp", "requesters", &json!({"email": "john@acme.com", "priority": "high"})).await;

    let (s, body) = rt.get_json("/api/v1/apps/crm_qp/collections/contacts?linked=true").await;
    assert_eq!(s, 200);
    let data = body.as_array().unwrap();
    let john = &data[0];
    assert_eq!(john["_linked"]["help_qp"]["data"]["priority"], "high");

    rt.shutdown().await;
}

#[tokio::test]
async fn identity_index_cleanup_on_removal() {
    let rt = TestRuntime::boot().await;

    // Install with identity
    rt.install_manifest(&json!({
        "appId": "idx_clean", "name": "Test", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "identityKind": "product", "identityKey": "sku", "fields": [
            { "name": "sku", "type": "text", "required": true }
        ]}]
    })).await;

    // Reinstall without identity
    rt.install_manifest(&json!({
        "appId": "idx_clean", "name": "Test", "version": "1.0.1",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "sku", "type": "text", "required": true }
        ]}]
    })).await;

    // Verify no _linked enrichment happens (identity was removed)
    rt.create("idx_clean", "items", &json!({"sku": "ABC"})).await;
    let (_, body) = rt.post_json("/api/v1/apps/idx_clean/collections/items/query", &json!({"linked": true})).await;
    let item = &body["data"].as_array().unwrap()[0];
    assert!(item.get("_linked").is_none(), "no identity = no linked enrichment");

    rt.shutdown().await;
}

#[tokio::test]
async fn identity_manifest_validation_rejects_invalid() {
    let rt = TestRuntime::boot().await;

    // identityKind without identityKey
    let (s, _) = rt.post_json("/api/v1/apps", &json!({
        "appId": "bad_id", "name": "Bad", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "identityKind": "product", "fields": [
            { "name": "sku", "type": "text" }
        ]}]
    })).await;
    assert_eq!(s, 500, "should reject identityKind without identityKey");

    // identityKey pointing to nonexistent field
    let (s2, _) = rt.post_json("/api/v1/apps", &json!({
        "appId": "bad_id2", "name": "Bad", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "identityKind": "product", "identityKey": "missing_field", "fields": [
            { "name": "sku", "type": "text" }
        ]}]
    })).await;
    assert_eq!(s2, 500, "should reject identityKey referencing nonexistent field");

    rt.shutdown().await;
}

#[tokio::test]
async fn identity_verify_detects_index_changes() {
    let rt = TestRuntime::boot().await;

    // Install without identity
    rt.install_manifest(&json!({
        "appId": "vfy_id", "name": "Test", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "sku", "type": "text", "required": true }
        ]}]
    })).await;

    // Verify with identity added — should detect missing index
    let (s, body) = rt.post_json("/api/v1/apps/schema/verify", &json!({
        "appId": "vfy_id", "name": "Test", "version": "1.0.1",
        "dataContract": [{ "entityName": "items", "identityKind": "product", "identityKey": "sku", "fields": [
            { "name": "sku", "type": "text", "required": true }
        ]}]
    })).await;
    assert_eq!(s, 200);
    let result: SchemaVerification = serde_json::from_value(body).unwrap();
    assert!(!result.compliant);
    assert!(result.changes.iter().any(|c| c.change_type == "add_identity_index" && c.entity == "items"));

    // Now install with identity
    rt.install_manifest(&json!({
        "appId": "vfy_id", "name": "Test", "version": "1.0.1",
        "dataContract": [{ "entityName": "items", "identityKind": "product", "identityKey": "sku", "fields": [
            { "name": "sku", "type": "text", "required": true }
        ]}]
    })).await;

    // Verify removing identity — should detect orphaned index
    let (_, body2) = rt.post_json("/api/v1/apps/schema/verify", &json!({
        "appId": "vfy_id", "name": "Test", "version": "1.0.2",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "sku", "type": "text", "required": true }
        ]}]
    })).await;
    let result2: SchemaVerification = serde_json::from_value(body2).unwrap();
    assert!(!result2.compliant);
    assert!(result2.changes.iter().any(|c| c.change_type == "drop_identity_index" && c.entity == "items"));

    rt.shutdown().await;
}

#[tokio::test]
async fn federated_query_across_apps() {
    let rt = TestRuntime::boot().await;

    rt.install_manifest(&json!({
        "appId": "crm_fed", "name": "CRM", "version": "1.0.0",
        "dataContract": [{ "entityName": "companies", "identityKind": "company", "identityKey": "name", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "industry", "type": "text" }
        ]}]
    })).await;

    rt.install_manifest(&json!({
        "appId": "bill_fed", "name": "Billing", "version": "1.0.0",
        "dataContract": [{ "entityName": "company", "identityKind": "company", "identityKey": "name", "fields": [
            { "name": "name", "type": "text", "required": true },
            { "name": "vat", "type": "text" }
        ]}]
    })).await;

    rt.create("crm_fed", "companies", &json!({"name": "Acme Corp", "industry": "Tech"})).await;
    rt.create("crm_fed", "companies", &json!({"name": "Beta Inc", "industry": "Finance"})).await;
    rt.create("bill_fed", "company", &json!({"name": "Acme Corp", "vat": "BE123"})).await;
    rt.create("bill_fed", "company", &json!({"name": "Gamma Ltd", "vat": "FR456"})).await;

    let (s, body) = rt.post_json("/api/v1/federated/company/query", &json!({})).await;
    assert_eq!(s, 200);
    let data = body["data"].as_array().unwrap();
    assert_eq!(body["total"], 4, "should return all companies from both apps");

    // Each record has _source
    for record in data {
        assert!(record["_source"]["app"].is_string(), "missing _source.app");
        assert!(record["_source"]["entity"].is_string(), "missing _source.entity");
    }

    let crm_records: Vec<_> = data.iter().filter(|r| r["_source"]["app"] == "crm_fed").collect();
    let bill_records: Vec<_> = data.iter().filter(|r| r["_source"]["app"] == "bill_fed").collect();
    assert_eq!(crm_records.len(), 2);
    assert_eq!(bill_records.len(), 2);

    // With where filter
    let (_, body2) = rt.post_json("/api/v1/federated/company/query", &json!({"where": {"name": "Acme Corp"}})).await;
    let data2 = body2["data"].as_array().unwrap();
    assert_eq!(data2.len(), 2, "Acme Corp exists in both apps");

    // Empty identityKind
    let (s3, body3) = rt.post_json("/api/v1/federated/nonexistent/query", &json!({})).await;
    assert_eq!(s3, 200);
    assert_eq!(body3["total"], 0);

    rt.shutdown().await;
}

// ── Migration integration tests ──────────────────────────────────────

#[tokio::test]
async fn migration_applies_on_deploy() {
    let rt = TestRuntime::boot().await;

    let manifest = json!({
        "appId": "migapp", "name": "MigApp", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "title", "type": "text", "required": true }
        ]}]
    });
    rt.install_manifest(&manifest).await;

    let migration_sql = format!(
        "CREATE SCHEMA IF NOT EXISTS \"migapp\";\n\
         CREATE TABLE IF NOT EXISTS \"migapp\".\"items\" (\n\
           \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(),\n\
           \"title\" TEXT NOT NULL,\n\
           \"created_at\" TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
           \"updated_at\" TIMESTAMPTZ NOT NULL DEFAULT now()\n\
         );"
    );
    let archive = make_tar_gz(&[
        ("migrations/001_initial.sql", migration_sql.as_bytes()),
        ("index.ts", b"process.stdin.resume();"),
    ]);
    let (s, body) = rt.deploy("migapp", &archive).await;
    assert_eq!(s, 200, "deploy with migrations should succeed: {body}");

    let (s, body) = rt.get_json("/api/v1/apps/migapp/collections/items").await;
    assert_eq!(s, 200, "table should exist after migration: {body}");

    rt.shutdown().await;
}

#[tokio::test]
async fn migration_skips_already_applied() {
    let rt = TestRuntime::boot().await;

    let manifest = json!({
        "appId": "migskip", "name": "MigSkip", "version": "1.0.0",
        "dataContract": [{ "entityName": "notes", "fields": [
            { "name": "body", "type": "text" },
            { "name": "tag", "type": "text" }
        ]}]
    });
    rt.install_manifest(&manifest).await;

    let sql1 = "CREATE SCHEMA IF NOT EXISTS \"migskip\";\n\
                CREATE TABLE IF NOT EXISTS \"migskip\".\"notes\" (\n\
                  \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(),\n\
                  \"body\" TEXT,\n\
                  \"created_at\" TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
                  \"updated_at\" TIMESTAMPTZ NOT NULL DEFAULT now()\n\
                );";

    let archive1 = make_tar_gz(&[
        ("migrations/001_create.sql", sql1.as_bytes()),
        ("index.ts", b"process.stdin.resume();"),
    ]);
    let (s, _) = rt.deploy("migskip", &archive1).await;
    assert_eq!(s, 200);

    rt.create("migskip", "notes", &json!({"body": "hello"})).await;

    let sql2 = "ALTER TABLE \"migskip\".\"notes\" ADD COLUMN IF NOT EXISTS \"tag\" TEXT;";
    let archive2 = make_tar_gz(&[
        ("migrations/001_create.sql", sql1.as_bytes()),
        ("migrations/002_add_tag.sql", sql2.as_bytes()),
        ("index.ts", b"process.stdin.resume();"),
    ]);
    let (s, body) = rt.deploy("migskip", &archive2).await;
    assert_eq!(s, 200, "second deploy failed: {body}");

    let (s, body) = rt.get_json("/api/v1/apps/migskip/collections/notes").await;
    assert_eq!(s, 200, "list notes failed: {body}");
    let records = body["data"].as_array().unwrap_or_else(|| body.as_array().unwrap());
    assert!(records.len() >= 1, "existing data preserved: {body}");

    rt.shutdown().await;
}

#[tokio::test]
async fn migration_rollback_on_failure() {
    let rt = TestRuntime::boot().await;

    let manifest = json!({
        "appId": "migfail", "name": "MigFail", "version": "1.0.0",
        "dataContract": [{ "entityName": "logs", "fields": [
            { "name": "msg", "type": "text" }
        ]}]
    });
    rt.install_manifest(&manifest).await;

    let sql1 = "CREATE SCHEMA IF NOT EXISTS \"migfail\";\n\
                CREATE TABLE IF NOT EXISTS \"migfail\".\"logs\" (\n\
                  \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(),\n\
                  \"msg\" TEXT,\n\
                  \"created_at\" TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
                  \"updated_at\" TIMESTAMPTZ NOT NULL DEFAULT now()\n\
                );";
    let sql2 = "INVALID SQL THAT WILL FAIL;";

    let archive = make_tar_gz(&[
        ("migrations/001_create.sql", sql1.as_bytes()),
        ("migrations/002_bad.sql", sql2.as_bytes()),
        ("index.ts", b"process.stdin.resume();"),
    ]);
    let (s, _) = rt.deploy("migfail", &archive).await;
    assert_eq!(s, 400, "bad migration should fail deploy");

    // 001 committed in its own tx, so table should exist
    let (s, _) = rt.get_json("/api/v1/apps/migfail/collections/logs").await;
    assert_eq!(s, 200, "001 should have committed independently");

    rt.shutdown().await;
}

#[tokio::test]
async fn deploy_without_migrations_preserves_tables() {
    let rt = TestRuntime::boot().await;

    let manifest = json!({
        "appId": "nomigr", "name": "NoMigr", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "name", "type": "text", "required": true }
        ]}]
    });
    rt.install_manifest(&manifest).await;

    let archive = make_tar_gz(&[("index.ts", b"process.stdin.resume();")]);
    let (s, _) = rt.deploy("nomigr", &archive).await;
    assert_eq!(s, 200);

    let record = rt.create("nomigr", "items", &json!({"name": "test"})).await;
    assert_eq!(record["name"], "test");

    rt.shutdown().await;
}

#[tokio::test]
async fn migration_with_dollar_quoted_block() {
    let rt = TestRuntime::boot().await;

    let manifest = json!({
        "appId": "migdq", "name": "MigDQ", "version": "1.0.0",
        "dataContract": [{ "entityName": "events", "fields": [
            { "name": "kind", "type": "text", "required": true },
            { "name": "seq", "type": "number" }
        ]}]
    });
    rt.install_manifest(&manifest).await;

    // Migration uses DO $$ ... $$ with semicolons inside the block
    let sql = "\
        CREATE SCHEMA IF NOT EXISTS \"migdq\";\n\
        CREATE TABLE IF NOT EXISTS \"migdq\".\"events\" (\n\
          \"id\" UUID PRIMARY KEY DEFAULT gen_random_uuid(),\n\
          \"kind\" TEXT NOT NULL,\n\
          \"seq\" DOUBLE PRECISION,\n\
          \"created_at\" TIMESTAMPTZ NOT NULL DEFAULT now(),\n\
          \"updated_at\" TIMESTAMPTZ NOT NULL DEFAULT now()\n\
        );\n\
        DO $$ BEGIN\n\
          CREATE SEQUENCE IF NOT EXISTS \"migdq\".\"events_seq\";\n\
        EXCEPTION WHEN duplicate_table THEN NULL;\n\
        END $$;";

    let archive = make_tar_gz(&[
        ("migrations/001_init.sql", sql.as_bytes()),
        ("index.ts", b"process.stdin.resume();"),
    ]);
    let (s, body) = rt.deploy("migdq", &archive).await;
    assert_eq!(s, 200, "deploy with dollar-quoted migration should succeed: {body}");

    let record = rt.create("migdq", "events", &json!({"kind": "click"})).await;
    assert!(record["id"].is_string());

    rt.shutdown().await;
}
