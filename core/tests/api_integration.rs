mod harness;
use harness::TestRuntime;
use reqwest::Method;
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
    assert_eq!(rt.get_unauthed("/api/v1/apps/authtest/crons").await, 401);
    assert_eq!(rt.post_unauthed("/api/v1/apps/authtest/crons", &json!({"name":"x","schedule":"* * * * *"})).await.0, 401);
    assert_eq!(rt.post_unauthed("/api/v1/apps/authtest/worker/start", &json!({})).await.0, 401);
    assert_eq!(rt.post_unauthed("/api/v1/apps/authtest/worker/stop", &json!({})).await.0, 401);
    assert_eq!(rt.delete_unauthed("/api/v1/users/00000000-0000-0000-0000-000000000099").await, 401);
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
async fn crud_unknown_entity_returns_404() {
    let rt = TestRuntime::boot().await;
    rt.install("unkapp", "contacts").await;
    let (s, body) = rt.get_json("/api/v1/apps/unkapp/collections/ghosts").await;
    assert_eq!(s, 404);
    assert!(body["error"].as_str().unwrap().contains("ghosts"), "should name entity: {body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn crud_namespaced_entity_returns_404() {
    let rt = TestRuntime::boot().await;
    rt.install("nsapp", "contacts").await;
    let (s, body) = rt.get_json("/api/v1/apps/nsapp/collections/core:users").await;
    assert_eq!(s, 404);
    assert!(body["error"].as_str().unwrap().contains("core:users"), "should name entity: {body}");
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
async fn jobs_enqueue_and_list() {
    let rt = TestRuntime::boot().await;
    rt.install("job", "items").await;

    let (s, body) = rt.post_json("/api/v1/apps/job/jobs", &json!({"payload":{"task":"csv"}})).await;
    assert_eq!(s, 201);
    assert!(body["msg_id"].as_i64().is_some());

    let (_, jobs) = rt.get_json("/api/v1/apps/job/jobs").await;
    let arr = jobs.as_array().unwrap();
    assert!(!arr.is_empty() || {
        // job may have been consumed already — check archive
        let (_, archived) = rt.get_json("/api/v1/apps/job/jobs?archived=true").await;
        !archived.as_array().unwrap().is_empty()
    });
    rt.shutdown().await;
}

#[tokio::test]
async fn jobs_list() {
    let rt = TestRuntime::boot().await;
    rt.install("jl", "items").await;
    for i in 0..3 {
        let (s, body) = rt.post_json("/api/v1/apps/jl/jobs", &json!({"payload":{"i":i}})).await;
        assert_eq!(s, 201);
        assert!(body["msg_id"].as_i64().is_some());
    }
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
async fn job_list_archived() {
    let rt = TestRuntime::boot().await;
    rt.install("jfilt", "items").await;

    rt.post_json("/api/v1/apps/jfilt/jobs", &json!({"payload":{"x":1}})).await;

    // Wait for scheduler to consume and archive/delete the job
    for _ in 0..30 {
        let (_, pending) = rt.get_json("/api/v1/apps/jfilt/jobs").await;
        if pending.as_array().unwrap().is_empty() {
            rt.shutdown().await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("job was not consumed within timeout");
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

    let (s, body) = rt.get_json("/api/v1/roles").await;
    assert_eq!(s, 200);
    let roles = body.as_array().unwrap();
    assert!(roles.iter().any(|r| r["name"] == "admin"), "should have built-in admin role");
    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_create_and_delete_role() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacc", "items").await;

    let (s, _) = rt.post_json("/api/v1/roles", &json!({
        "name": "editor",
        "description": "Can edit items",
        "permissions": ["app:rbacc:items.read", "app:rbacc:items.update"]
    })).await;
    assert_eq!(s, 200);

    let (_, roles) = rt.get_json("/api/v1/roles").await;
    assert!(roles.as_array().unwrap().iter().any(|r| r["name"] == "editor"));

    assert_eq!(rt.delete("/api/v1/roles/editor").await, 200);

    let (_, roles) = rt.get_json("/api/v1/roles").await;
    assert!(!roles.as_array().unwrap().iter().any(|r| r["name"] == "editor"));
    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_cannot_create_admin_role() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacadm", "items").await;

    let (s, _) = rt.post_json("/api/v1/roles", &json!({"name":"admin"})).await;
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
    rt.post_json("/api/v1/roles", &json!({"name":"viewer","permissions":["app:rbacas:items.read"]})).await;

    // assign
    let (s, _) = rt.post_json("/api/v1/roles/assign", &json!({"userId": user_id, "role": "viewer"})).await;
    assert_eq!(s, 200);

    // list assignments
    let (s, body) = rt.get_json("/api/v1/roles/assignments").await;
    assert_eq!(s, 200);
    assert!(body.as_array().unwrap().iter().any(|a| a["role"] == "viewer"));

    // revoke
    let (s, _) = rt.post_json("/api/v1/roles/revoke", &json!({"userId": user_id, "role": "viewer"})).await;
    assert_eq!(s, 200);

    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_my_permissions() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacperm", "items").await;

    let (s, body) = rt.get_json("/api/v1/permissions").await;
    assert_eq!(s, 200);
    let roles: Vec<&str> = body["roles"].as_array().unwrap().iter().filter_map(|r| r.as_str()).collect();
    assert!(roles.contains(&"admin"), "first user should be admin");
    rt.shutdown().await;
}

#[tokio::test]
async fn rbac_available_permissions() {
    let rt = TestRuntime::boot().await;
    rt.install("rbacavail", "items").await;

    let (s, body) = rt.get_json("/api/v1/permissions/available").await;
    assert_eq!(s, 200);
    let perms = body.as_array().unwrap();
    let keys: Vec<&str> = perms.iter().filter_map(|p| p["key"].as_str()).collect();
    // Permissions are namespaced: app:{app_id}:{entity}.{action}
    assert!(keys.contains(&"app:rbacavail:items.read"));
    assert!(keys.contains(&"app:rbacavail:items.create"));
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

// ── OIDC ────────────────────────────────────────────────────────────────────

/// Seed a provider directly in DB (bypasses discovery validation).
async fn seed_oidc_provider(pool: &sqlx::PgPool, id: &str, display_name: &str, enabled: bool) {
    sqlx::query(
        "INSERT INTO rootcx_system.oidc_providers
            (id, display_name, issuer_url, client_id, scopes, auto_register, default_role, enabled)
         VALUES ($1, $2, 'https://fake.example.com', 'cid', '{openid}', true, 'admin', $3)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(display_name)
    .bind(enabled)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn oidc_auth_mode_includes_providers() {
    let rt = TestRuntime::boot().await;

    // Baseline: no providers
    let (s, body) = rt.get_json("/api/v1/auth/mode").await;
    assert_eq!(s, 200);
    assert!(body["providers"].as_array().unwrap().is_empty());
    assert_eq!(body["passwordLoginEnabled"], true);

    // Seed one provider
    seed_oidc_provider(rt.pool(), "acme", "Acme SSO", true).await;
    let (_, body) = rt.get_json("/api/v1/auth/mode").await;
    let providers = body["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["id"], "acme");
    assert_eq!(providers[0]["displayName"], "Acme SSO");

    // Disabled provider is excluded
    seed_oidc_provider(rt.pool(), "hidden", "Hidden", false).await;
    let (_, body) = rt.get_json("/api/v1/auth/mode").await;
    assert_eq!(body["providers"].as_array().unwrap().len(), 1, "disabled provider must not appear");

    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_list_providers_is_public() {
    let rt = TestRuntime::boot().await;

    // Unauthenticated access should work (login screen needs this)
    let r = rt.client.get(rt.url("/api/v1/auth/oidc/providers")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty());

    seed_oidc_provider(rt.pool(), "corp", "Corp IdP", true).await;
    seed_oidc_provider(rt.pool(), "disabled", "Off", false).await;

    let r = rt.client.get(rt.url("/api/v1/auth/oidc/providers")).send().await.unwrap();
    let body: Value = r.json().await.unwrap();
    let ids: Vec<&str> = body.as_array().unwrap().iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["corp"], "only enabled providers are listed");

    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_upsert_provider_validation() {
    let rt = TestRuntime::boot().await;

    for (label, body, expected_status) in [
        ("empty id", json!({"id":"","displayName":"X","issuerUrl":"https://x.com","clientId":"c"}), 400),
        ("empty issuer", json!({"id":"x","displayName":"X","issuerUrl":"","clientId":"c"}), 400),
        ("empty client_id", json!({"id":"x","displayName":"X","issuerUrl":"https://x.com","clientId":""}), 400),
        ("http non-localhost", json!({"id":"x","displayName":"X","issuerUrl":"http://evil.com","clientId":"c"}), 400),
    ] {
        let (s, _) = rt.post_json("/api/v1/auth/oidc/providers", &body).await;
        assert_eq!(s, expected_status, "upsert validation: {label}");
    }

    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_upsert_provider_rejects_unauthenticated() {
    let rt = TestRuntime::boot().await;
    let body = json!({"id":"x","displayName":"X","issuerUrl":"https://x.com","clientId":"c"});
    let (s, _) = rt.post_unauthed("/api/v1/auth/oidc/providers", &body).await;
    assert_eq!(s, 401);
    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_delete_provider_lifecycle() {
    let rt = TestRuntime::boot().await;

    // Delete nonexistent → 404
    let (s, _) = rt.delete_json("/api/v1/auth/oidc/providers/ghost").await;
    assert_eq!(s, 404, "deleting nonexistent provider should 404");

    // Seed and delete
    seed_oidc_provider(rt.pool(), "todel", "To Delete", true).await;
    let (s, _) = rt.delete_json("/api/v1/auth/oidc/providers/todel").await;
    assert_eq!(s, 200);

    // Verify gone
    let r = rt.client.get(rt.url("/api/v1/auth/oidc/providers")).send().await.unwrap();
    let body: Value = r.json().await.unwrap();
    assert!(body.as_array().unwrap().is_empty(), "provider should be deleted");

    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_delete_provider_rejects_unauthenticated() {
    let rt = TestRuntime::boot().await;
    let s = rt.delete_unauthed("/api/v1/auth/oidc/providers/anything").await;
    assert_eq!(s, 401);
    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_token_exchange_unknown_provider() {
    let rt = TestRuntime::boot().await;
    let (s, body) = rt.post_json("/api/v1/auth/oidc/token-exchange", &json!({
        "providerId": "nonexistent",
        "idToken": "fake.jwt.here",
    })).await;
    assert_eq!(s, 404, "token exchange with unknown provider: {body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_token_exchange_invalid_id_token() {
    let rt = TestRuntime::boot().await;
    seed_oidc_provider(rt.pool(), "testidp", "Test IdP", true).await;

    let (s, _) = rt.post_json("/api/v1/auth/oidc/token-exchange", &json!({
        "providerId": "testidp",
        "idToken": "not-a-valid-jwt",
    })).await;
    // Discovery fetch for fake issuer will fail → 500
    assert!(s.is_client_error() || s.is_server_error(), "invalid id_token should fail");
    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_authorize_unknown_provider() {
    let rt = TestRuntime::boot().await;
    let r = rt.client
        .get(rt.url("/api/v1/auth/oidc/ghost/authorize"))
        .send().await.unwrap();
    // Provider not found → error (404 or 500 from discovery failure)
    assert!(r.status().is_client_error() || r.status().is_server_error());
    rt.shutdown().await;
}

#[tokio::test]
async fn oidc_callback_rejects_invalid_state() {
    let rt = TestRuntime::boot().await;
    let r = rt.client
        .get(rt.url("/api/v1/auth/oidc/callback?code=fake&state=bogus"))
        .send().await.unwrap();
    assert_eq!(r.status(), 401, "callback with unknown state should be rejected");
    rt.shutdown().await;
}

// ── Delete User ──

#[tokio::test]
async fn delete_user_removes_from_list_and_cascades_assignments() {
    let rt = TestRuntime::boot().await;

    rt.post_unauthed(
        "/api/v1/auth/register",
        &json!({"email":"victim@test.local","password":"Str0ngPass1"}),
    ).await;

    let (_, users) = rt.get_json("/api/v1/users").await;
    let victim_id = users.as_array().unwrap().iter()
        .find(|u| u["email"] == "victim@test.local").unwrap()["id"]
        .as_str().unwrap().to_string();

    // Assign a role so we can verify ON DELETE CASCADE cleans up rbac_assignments
    rt.post_json("/api/v1/roles/assign", &json!({"userId": victim_id, "role": "admin"})).await;

    let (s, _) = rt.delete_json(&format!("/api/v1/users/{victim_id}")).await;
    assert_eq!(s, 200);

    let (_, users_after) = rt.get_json("/api/v1/users").await;
    assert!(
        !users_after.as_array().unwrap().iter().any(|u| u["email"] == "victim@test.local"),
        "deleted user must not appear in GET /api/v1/users"
    );

    let (_, assignments) = rt.get_json("/api/v1/roles/assignments").await;
    assert!(
        !assignments.as_array().unwrap().iter().any(|a| a["userId"] == victim_id.as_str()),
        "deleted user must have no role assignments"
    );

    rt.shutdown().await;
}

#[tokio::test]
async fn delete_user_rejects_unauthorized_and_unsafe_operations() {
    let rt = TestRuntime::boot().await;

    rt.post_unauthed(
        "/api/v1/auth/register",
        &json!({"email":"nonadmin@test.local","password":"Str0ngPass1"}),
    ).await;
    let (_, login) = rt.post_unauthed(
        "/api/v1/auth/login",
        &json!({"email":"nonadmin@test.local","password":"Str0ngPass1"}),
    ).await;
    let nonadmin_token = login["accessToken"].as_str().unwrap();

    let (_, users) = rt.get_json("/api/v1/users").await;
    let admin_id = users.as_array().unwrap().iter()
        .find(|u| u["email"] == "admin@test.local").unwrap()["id"]
        .as_str().unwrap().to_string();

    let cases: &[(&str, &str, &str, u16)] = &[
        ("non-admin cannot delete",  nonadmin_token, &admin_id,                                  403),
        ("cannot delete last admin",  &rt.token,     &admin_id,                                  400),
        ("nonexistent user returns 404", &rt.token,  "00000000-0000-0000-0000-ffffffffffff",     404),
    ];

    for &(label, token, target_id, expected) in cases {
        let r = rt.client
            .delete(rt.url(&format!("/api/v1/users/{target_id}")))
            .bearer_auth(token)
            .send().await.unwrap();
        assert_eq!(r.status().as_u16(), expected, "{label}");
    }

    rt.shutdown().await;
}

// Verifies the `core:users` entity_link DSL produces a real, enforced FK
// against rootcx_system.users with ON DELETE SET NULL.
#[tokio::test]
async fn entity_link_core_users_fk_is_enforced() {
    let rt = TestRuntime::boot().await;

    let manifest = json!({
        "appId": "tasks", "name": "tasks", "version": "1.0.0",
        "dataContract": [{
            "entityName": "tickets",
            "fields": [
                { "name": "title", "type": "text", "required": true },
                { "name": "owner_id", "type": "entity_link",
                  "references": { "entity": "core:users", "field": "id" } }
            ]
        }]
    });
    rt.install_manifest(&manifest).await;

    let owner_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.users (email, is_system) VALUES ($1, false) RETURNING id"
    )
    .bind("ticket-owner@test.local")
    .fetch_one(rt.pool()).await.unwrap();

    let ticket = rt.create("tasks", "tickets", &json!({
        "title": "triage inbox",
        "owner_id": owner_id.to_string(),
    })).await;
    let ticket_id: uuid::Uuid = ticket["id"].as_str().unwrap().parse().unwrap();

    let (status, _) = rt.post_json("/api/v1/apps/tasks/collections/tickets", &json!({
        "title": "bad ref",
        "owner_id": uuid::Uuid::new_v4().to_string(),
    })).await;
    assert!(!status.is_success(), "expected FK violation, got {status} with bogus owner_id");

    sqlx::query("DELETE FROM rootcx_system.users WHERE id = $1")
        .bind(owner_id).execute(rt.pool()).await.unwrap();

    let owner_after: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT owner_id FROM tasks.tickets WHERE id = $1"
    )
    .bind(ticket_id)
    .fetch_one(rt.pool()).await.unwrap();
    assert!(owner_after.is_none(), "owner_id should be NULL after referenced user deleted");

    rt.shutdown().await;
}

// ── IPC Protocol v1/v2 Integration ──────────────────────────────────────────
// These tests verify the real Rust supervisor ↔ Bun worker boundary through
// the injected prelude. Unit tests (backend_prelude.test.ts) cover the JS
// dispatch logic in isolation; these tests cover what they structurally
// cannot: wire-format agreement, protocol negotiation, and collection_op
// round-trips through the Rust layer.

#[tokio::test]
async fn ipc_v2_rpc_round_trip() {
    let rt = TestRuntime::boot().await;
    rt.install("ipcv2", "items").await;

    let backend = br#"
        serve({
            rpc: {
                ping: (params) => ({ reply: "pong", echo: params.msg }),
            },
        });
    "#;
    let archive = make_tar_gz(&[("index.ts", backend)]);
    let (s, _) = rt.deploy("ipcv2", &archive).await;
    assert_eq!(s, 200);

    // Worker needs a moment to complete the discover handshake.
    let mut result = json!(null);
    for _ in 0..20 {
        let (s, body) = rt.post_json("/api/v1/apps/ipcv2/rpc", &json!({
            "method": "ping", "params": {"msg": "hello"}
        })).await;
        if s == 200 {
            result = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(result["reply"], "pong");
    assert_eq!(result["echo"], "hello");
    rt.shutdown().await;
}

#[tokio::test]
async fn ipc_v2_collection_op_round_trip() {
    let rt = TestRuntime::boot().await;
    rt.install("ipcco", "items").await;

    let backend = br#"
        serve({
            rpc: {
                async create_item(params, _caller, ctx) {
                    const row = await ctx.collection("items").insert({
                        first_name: params.first_name,
                        last_name: params.last_name,
                    });
                    return { id: row.id };
                },
            },
        });
    "#;
    let archive = make_tar_gz(&[("index.ts", backend)]);
    let (s, _) = rt.deploy("ipcco", &archive).await;
    assert_eq!(s, 200);

    let mut rpc_result = json!(null);
    for _ in 0..20 {
        let (s, body) = rt.post_json("/api/v1/apps/ipcco/rpc", &json!({
            "method": "create_item",
            "params": {"first_name": "Alice", "last_name": "Martin"}
        })).await;
        if s == 200 {
            rpc_result = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(rpc_result["id"].is_string(), "RPC should return inserted row id: {rpc_result}");

    // Verify the record exists in the database via REST API
    let (s, rows) = rt.get_json("/api/v1/apps/ipcco/collections/items").await;
    assert_eq!(s, 200);
    let items = rows.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["first_name"], "Alice");
    assert_eq!(items[0]["last_name"], "Martin");
    rt.shutdown().await;
}

#[tokio::test]
async fn ipc_v1_legacy_worker_with_new_prelude() {
    let rt = TestRuntime::boot().await;
    rt.install("ipcv1", "items").await;

    // Legacy backend: manual stdin handler, no serve(). Must still work
    // with the new prelude injected via --preload (prelude stays silent on
    // discover/rpc when _handlers is null).
    let backend = br#"
        const write = (m) => process.stdout.write(JSON.stringify(m) + "\n");
        process.stdin.setEncoding("utf-8");
        let buf = "";
        process.stdin.on("data", (chunk) => {
            buf += chunk;
            let nl;
            while ((nl = buf.indexOf("\n")) !== -1) {
                const line = buf.slice(0, nl).trim();
                buf = buf.slice(nl + 1);
                if (!line) continue;
                const msg = JSON.parse(line);
                if (msg.type === "discover") write({ type: "discover", methods: ["ping"] });
                if (msg.type === "rpc" && msg.method === "ping")
                    write({ type: "rpc_response", id: msg.id, result: { reply: "legacy_pong" } });
                if (msg.type === "shutdown") process.exit(0);
            }
        });
    "#;
    let archive = make_tar_gz(&[("index.ts", backend)]);
    let (s, _) = rt.deploy("ipcv1", &archive).await;
    assert_eq!(s, 200);

    let mut result = json!(null);
    for _ in 0..20 {
        let (s, body) = rt.post_json("/api/v1/apps/ipcv1/rpc", &json!({
            "method": "ping", "params": {}
        })).await;
        if s == 200 {
            result = body;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert_eq!(result["reply"], "legacy_pong", "v1 legacy worker must still work: {result}");
    rt.shutdown().await;
}

// ── Cron schedule tests ─────────────────────────────────────────────

#[tokio::test]
async fn cron_crud_lifecycle() {
    let rt = TestRuntime::boot().await;
    rt.install("cronapp", "items").await;

    // Create
    let (s, body) = rt.post_json("/api/v1/apps/cronapp/crons", &json!({
        "name": "daily-sync",
        "schedule": "0 8 * * *",
        "payload": { "method": "syncAll" },
        "overlapPolicy": "skip"
    })).await;
    assert_eq!(s, 201, "create cron: {body}");
    let cron_id = body["id"].as_str().expect("id missing");
    assert_eq!(body["name"], "daily-sync");
    assert_eq!(body["schedule"], "0 8 * * *");
    assert_eq!(body["overlapPolicy"], "skip");
    assert!(body["pgJobId"].as_i64().is_some(), "pg_cron job should be assigned: {body}");

    // List
    let (s, list) = rt.get_json("/api/v1/apps/cronapp/crons").await;
    assert_eq!(s, 200);
    let arr = list.as_array().expect("should be array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "daily-sync");

    // Update
    let (s, updated) = rt.patch_json(
        &format!("/api/v1/apps/cronapp/crons/{cron_id}"),
        &json!({ "schedule": "*/5 * * * *", "overlapPolicy": "queue" }),
    ).await;
    assert_eq!(s, 200, "update cron: {updated}");
    assert_eq!(updated["schedule"], "*/5 * * * *");
    assert_eq!(updated["overlapPolicy"], "queue");

    // Trigger
    let (s, trig) = rt.post_json(
        &format!("/api/v1/apps/cronapp/crons/{cron_id}/trigger"),
        &json!({}),
    ).await;
    assert_eq!(s, 200, "trigger cron: {trig}");
    assert!(trig["msgId"].as_i64().is_some(), "should return pgmq msg id: {trig}");

    // Disable
    let (s, disabled) = rt.patch_json(
        &format!("/api/v1/apps/cronapp/crons/{cron_id}"),
        &json!({ "enabled": false }),
    ).await;
    assert_eq!(s, 200, "disable: {disabled}");
    assert_eq!(disabled["enabled"], false);
    assert!(disabled["pgJobId"].is_null(), "disabled cron should have no pg_cron job: {disabled}");

    // Re-enable
    let (s, reenabled) = rt.patch_json(
        &format!("/api/v1/apps/cronapp/crons/{cron_id}"),
        &json!({ "enabled": true }),
    ).await;
    assert_eq!(s, 200);
    assert!(reenabled["pgJobId"].as_i64().is_some(), "re-enabled should reassign pg_cron: {reenabled}");

    // Delete
    let s = rt.delete(&format!("/api/v1/apps/cronapp/crons/{cron_id}")).await;
    assert_eq!(s, 200, "delete cron");

    // Verify empty
    let (_, list) = rt.get_json("/api/v1/apps/cronapp/crons").await;
    assert_eq!(list.as_array().unwrap().len(), 0);

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_validation_returns_400() {
    let rt = TestRuntime::boot().await;
    rt.install("cronval", "items").await;

    let cases = vec![
        (json!({"name": "ok", "schedule": "bad schedule"}), "invalid schedule"),
        (json!({"name": "semi;colon", "schedule": "0 * * * *"}), "invalid name"),
        (json!({"name": "ok", "schedule": "0 * * * *", "overlapPolicy": "nope"}), "invalid overlap"),
        (json!({"schedule": "0 * * * *"}), "missing name"),
        (json!({"name": "ok"}), "missing schedule"),
    ];
    for (body, label) in cases {
        let (s, _) = rt.post_json("/api/v1/apps/cronval/crons", &body).await;
        assert_eq!(s, 400, "{label} should return 400");
    }

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_blocked_for_system_schemas() {
    let rt = TestRuntime::boot().await;
    let (s, _) = rt.get_json("/api/v1/apps/rootcx_system/crons").await;
    assert_eq!(s, 403, "rootcx_system should be blocked");
    let (s, _) = rt.get_json("/api/v1/apps/pg_catalog/crons").await;
    assert_eq!(s, 403, "pg_catalog should be blocked");
    rt.shutdown().await;
}

#[tokio::test]
async fn cron_cleanup_on_uninstall() {
    let rt = TestRuntime::boot().await;
    rt.install("cronclean", "items").await;

    let (s, _) = rt.post_json("/api/v1/apps/cronclean/crons", &json!({
        "name": "job1", "schedule": "0 * * * *"
    })).await;
    assert_eq!(s, 201);

    let s = rt.delete("/api/v1/apps/cronclean").await;
    assert_eq!(s, 200, "uninstall should succeed");

    // Verify pg_cron jobs cleaned up
    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM cron.job WHERE jobname LIKE 'rootcx-%'"
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(count, 0, "pg_cron jobs should be cleaned up after uninstall");

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_manifest_sync() {
    let rt = TestRuntime::boot().await;
    let manifest = json!({
        "appId": "cronsync", "name": "cronsync", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "label", "type": "text" }
        ]}],
        "crons": [
            { "name": "hourly", "schedule": "0 * * * *", "method": "tick" },
            { "name": "nightly", "schedule": "0 2 * * *", "overlapPolicy": "queue" }
        ]
    });
    rt.install_manifest(&manifest).await;

    let (s, list) = rt.get_json("/api/v1/apps/cronsync/crons").await;
    assert_eq!(s, 200);
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 2, "manifest declared 2 crons: {list}");

    let hourly = arr.iter().find(|c| c["name"] == "hourly").expect("hourly missing");
    assert_eq!(hourly["payload"]["method"], "tick", "method should be in payload: {hourly}");
    let nightly = arr.iter().find(|c| c["name"] == "nightly").expect("nightly missing");
    assert_eq!(nightly["overlapPolicy"], "queue");

    // Re-install with one cron removed — should delete the orphan
    let manifest2 = json!({
        "appId": "cronsync", "name": "cronsync", "version": "1.0.1",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "label", "type": "text" }
        ]}],
        "crons": [
            { "name": "hourly", "schedule": "*/30 * * * *", "method": "tick" }
        ]
    });
    rt.install_manifest(&manifest2).await;

    let (_, list2) = rt.get_json("/api/v1/apps/cronsync/crons").await;
    let arr2 = list2.as_array().unwrap();
    assert_eq!(arr2.len(), 1, "orphaned 'nightly' should be deleted: {list2}");
    assert_eq!(arr2[0]["schedule"], "*/30 * * * *", "hourly should be updated: {list2}");

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_job_carries_creator_user_id() {
    let rt = TestRuntime::boot().await;
    rt.install("cronuser", "items").await;

    // Create cron — the authenticated user should be stored as created_by
    let (s, body) = rt.post_json("/api/v1/apps/cronuser/crons", &json!({
        "name": "sync-job", "schedule": "0 * * * *",
        "payload": { "type": "sync_replies" }
    })).await;
    assert_eq!(s, 201, "create: {body}");
    let cron_id = body["id"].as_str().unwrap();
    assert!(body["createdBy"].is_string(), "createdBy should be set on create: {body}");

    // Trigger → enqueues a pgmq message
    let (s, trig) = rt.post_json(
        &format!("/api/v1/apps/cronuser/crons/{cron_id}/trigger"),
        &json!({}),
    ).await;
    assert_eq!(s, 200, "trigger: {trig}");
    let msg_id = trig["msgId"].as_i64().unwrap();

    // Read the raw pgmq message and verify user_id is present
    let (raw,): (serde_json::Value,) = sqlx::query_as(
        "SELECT message FROM pgmq.q_jobs WHERE msg_id = $1"
    ).bind(msg_id).fetch_one(rt.pool()).await.unwrap();

    assert!(raw.get("user_id").is_some(), "pgmq job message must contain user_id: {raw}");
    assert_eq!(
        raw["user_id"].as_str().unwrap(),
        body["createdBy"].as_str().unwrap(),
        "user_id in job must match cron creator"
    );

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_manifest_sync_stores_created_by() {
    let rt = TestRuntime::boot().await;
    let manifest = json!({
        "appId": "cronown", "name": "cronown", "version": "1.0.0",
        "dataContract": [{ "entityName": "items", "fields": [
            { "name": "label", "type": "text" }
        ]}],
        "crons": [
            { "name": "bg-job", "schedule": "*/5 * * * *", "method": "run" }
        ]
    });
    rt.install_manifest(&manifest).await;

    let (s, list) = rt.get_json("/api/v1/apps/cronown/crons").await;
    assert_eq!(s, 200);
    let arr = list.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    // Manifest install is authenticated — created_by should be the installing user
    assert!(arr[0]["createdBy"].is_string(), "manifest-synced cron should have createdBy: {}", arr[0]);

    rt.shutdown().await;
}

// ── Cron Security ──────────────────────────────────────────────────

#[tokio::test]
async fn cron_routes_require_permission() {
    let rt = TestRuntime::boot().await;
    rt.install("cronperm", "items").await;

    let (s, cron) = rt.post_json("/api/v1/apps/cronperm/crons", &json!({
        "name": "sec-job", "schedule": "0 * * * *", "payload": {}
    })).await;
    assert_eq!(s, 201);
    let cron_id = cron["id"].as_str().unwrap();

    let nobody = rt.register_and_login("nobody@test.local").await;

    let cron_url = format!("/api/v1/apps/cronperm/crons/{cron_id}");
    let cases: &[(&str, Method, String, Option<Value>)] = &[
        ("list",    Method::GET,    "/api/v1/apps/cronperm/crons".into(),   None),
        ("create",  Method::POST,   "/api/v1/apps/cronperm/crons".into(),   Some(json!({"name":"x","schedule":"0 * * * *"}))),
        ("update",  Method::PATCH,  cron_url.clone(),                       Some(json!({"schedule":"*/5 * * * *"}))),
        ("trigger", Method::POST,   format!("{cron_url}/trigger"),           Some(json!({}))),
        ("delete",  Method::DELETE, cron_url.clone(),                        None),
    ];
    for (label, method, path, body) in cases {
        let (s, _) = rt.request_as(method.clone(), path, &nobody, body.as_ref()).await;
        assert_eq!(s, 403, "{label}: user without permissions must be denied");
    }

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_non_owner_cannot_update_or_trigger() {
    let rt = TestRuntime::boot().await;
    rt.install("cronown2", "items").await;

    rt.post_json("/api/v1/roles", &json!({
        "name": "cron-user",
        "permissions": ["app:cronown2:cron.read", "app:cronown2:cron.write", "app:cronown2:cron.trigger"]
    })).await;

    let alice = rt.register_and_login("alice@test.local").await;
    let bob = rt.register_and_login("bob@test.local").await;

    let (_, users) = rt.get_json("/api/v1/users").await;
    for email in ["alice@test.local", "bob@test.local"] {
        let uid = users.as_array().unwrap().iter()
            .find(|u| u["email"] == email).unwrap()["id"].as_str().unwrap();
        rt.post_json("/api/v1/roles/assign", &json!({"userId": uid, "role": "cron-user"})).await;
    }

    let (s, cron) = rt.request_as(
        Method::POST, "/api/v1/apps/cronown2/crons", &alice,
        Some(&json!({"name": "alice-job", "schedule": "0 * * * *", "payload": {}})),
    ).await;
    assert_eq!(s, 201, "alice creates her cron");

    let cron_url = format!("/api/v1/apps/cronown2/crons/{}", cron["id"].as_str().unwrap());
    let cases: &[(&str, Method, String, Option<Value>)] = &[
        ("update",  Method::PATCH,  cron_url.clone(),              Some(json!({"schedule":"*/5 * * * *"}))),
        ("trigger", Method::POST,   format!("{cron_url}/trigger"), Some(json!({}))),
        ("delete",  Method::DELETE, cron_url.clone(),               None),
    ];
    for (label, method, path, body) in cases {
        let (s, _) = rt.request_as(method.clone(), path, &bob, body.as_ref()).await;
        assert_eq!(s, 403, "bob {label} alice's cron without manage_others must be forbidden");
    }

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_created_by_immutable_via_update() {
    let rt = TestRuntime::boot().await;
    rt.install("cronimm", "items").await;

    let (s, cron) = rt.post_json("/api/v1/apps/cronimm/crons", &json!({
        "name": "imm-job", "schedule": "0 * * * *", "payload": {}
    })).await;
    assert_eq!(s, 201);
    let cron_id = cron["id"].as_str().unwrap();
    let original = cron["createdBy"].as_str().unwrap();

    let (_, updated) = rt.patch_json(
        &format!("/api/v1/apps/cronimm/crons/{cron_id}"),
        &json!({"createdBy": "00000000-0000-0000-0000-ffffffffffff"}),
    ).await;

    assert_eq!(
        updated["createdBy"].as_str().unwrap(), original,
        "created_by must not change via update body"
    );

    rt.shutdown().await;
}

#[tokio::test]
async fn cron_created_by_nulled_on_user_deletion() {
    let rt = TestRuntime::boot().await;
    rt.install("cronorphan", "items").await;

    let doomed = rt.register_and_login("doomed@test.local").await;
    let (_, users) = rt.get_json("/api/v1/users").await;
    let doomed_id = users.as_array().unwrap().iter()
        .find(|u| u["email"] == "doomed@test.local").unwrap()["id"]
        .as_str().unwrap().to_string();

    rt.post_json("/api/v1/roles/assign", &json!({"userId": doomed_id, "role": "admin"})).await;

    let (s, cron) = rt.request_as(
        Method::POST, "/api/v1/apps/cronorphan/crons", &doomed,
        Some(&json!({"name": "orphan-job", "schedule": "0 * * * *", "payload": {}})),
    ).await;
    assert_eq!(s, 201);
    let cron_id = cron["id"].as_str().unwrap();
    assert_eq!(cron["createdBy"].as_str().unwrap(), doomed_id);

    let (s, _) = rt.delete_json(&format!("/api/v1/users/{doomed_id}")).await;
    assert_eq!(s, 200);

    let (created_by,): (Option<uuid::Uuid>,) = sqlx::query_as(
        "SELECT created_by FROM rootcx_system.cron_schedules WHERE id = $1::uuid"
    ).bind(cron_id).fetch_one(rt.pool()).await.unwrap();

    assert!(created_by.is_none(), "created_by must be NULL after user deletion, got: {created_by:?}");

    rt.shutdown().await;
}

