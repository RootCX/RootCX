mod harness;
use axum::{Json, Router, body::Bytes, http::StatusCode, routing::put};
use base64::{Engine, engine::general_purpose::STANDARD};
use harness::TestRuntime;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};

static BACKUPS: AtomicUsize = AtomicUsize::new(0);

fn sources(version: &str, description: &str) -> Value {
    let manifest = json!({"appId":"recovery_test","name":"Recovery Test","version":version,
        "dataContract":[{"entityName":"items","fields":[{"name":"label","type":"text"}]}],
        "actions":[{"id":"create_item","name":"Create item","description":description}]});
    json!({"manifest.json":STANDARD.encode(serde_json::to_vec(&manifest).unwrap()),"src/App.tsx":STANDARD.encode("export const app = 1;")})
}

#[test]
fn interrupted_upgrade_publishes_nothing_and_resumes_on_retry() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    unsafe {
        std::env::set_var(
            "ROOTCX_BUILDER_BACKUP_URL",
            format!("http://127.0.0.1:{}", addr.port()),
        );
        std::env::set_var("ROOTCX_BUILDER_ALLOW_LOCAL_HTTP", "true");
        std::env::set_var(
            "ROOTCX_BUILDER_BACKUP_TOKEN",
            "backup-recovery-token-000000000000",
        );
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async move {
        // Import succeeds, the upgrade's first backup fails, every later one succeeds.
        let receiver = tokio::spawn(async move {
            axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), Router::new().route("/{app}/{file}", put(|body: Bytes| async move {
                if BACKUPS.fetch_add(1, Ordering::SeqCst) == 1 {
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
                Ok(Json(json!({"sha256":hex::encode(Sha256::digest(&body)),"bytes":body.len()})))
            }))).await.unwrap();
        });
        let rt = TestRuntime::boot().await;
        let v1 = sources("1.0.0", "");
        let manifest: Value = serde_json::from_slice(&STANDARD.decode(v1["manifest.json"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(rt.post_json("/api/v1/apps", &manifest).await.0, 200);
        let (status, imported) = rt.post_json("/api/v1/apps/recovery_test/sources", &json!({"files":v1})).await;
        assert_eq!(status, 200, "{imported}");
        let head = imported["headCommit"].as_str().unwrap();
        let frontend = STANDARD.encode(harness::make_tar_gz(&[("index.html", b"<html>v2</html>".as_slice())]));
        let request = json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":head,"files":sources("2.0.0", "Creates an item."),"frontend":frontend});

        let (status, body) = rt.post_json("/api/v1/apps/recovery_test/sources/upgrade", &request).await;
        assert_eq!(status, 503, "{body}");
        let (_, unchanged) = rt.get_json("/api/v1/apps/recovery_test/sources").await;
        assert_eq!(unchanged["headCommit"], head, "nothing is published before the backup is acknowledged");
        let version: String = sqlx::query_scalar("SELECT manifest->>'version' FROM rootcx_system.apps WHERE id='recovery_test'").fetch_one(rt.pool()).await.unwrap();
        assert_eq!(version, "1.0.0");
        let other = json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":head,"files":sources("3.0.0", "x"),"frontend":frontend});
        assert_eq!(rt.post_json("/api/v1/apps/recovery_test/sources/upgrade", &other).await.0, 409, "an interrupted upgrade blocks other changes");

        let (status, run) = rt.post_json("/api/v1/apps/recovery_test/sources/upgrade", &request).await;
        assert_eq!(status, 200, "{run}");
        assert_eq!(run["status"], "succeeded", "{run}");
        let (_, sources_state) = rt.get_json("/api/v1/apps/recovery_test/sources").await;
        assert_eq!(sources_state["headCommit"], run["commitId"]);
        assert_eq!(sources_state["deployedCommit"], run["commitId"]);
        let version: String = sqlx::query_scalar("SELECT manifest->>'version' FROM rootcx_system.apps WHERE id='recovery_test'").fetch_one(rt.pool()).await.unwrap();
        assert_eq!(version, "2.0.0");
        let (status, body) = rt.post_json("/api/v1/apps/recovery_test/sources/upgrade",
            &json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":run["commitId"],"files":sources("2.0.0", "Creates an item."),"frontend":frontend})).await;
        assert_eq!(status, 409, "the recovered release must be recorded as applied: {body}");
        receiver.abort();
        rt.shutdown().await;
    });
}
