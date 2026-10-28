mod harness;
use harness::TestRuntime;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

async fn prepare(rt: &TestRuntime, app: &str) -> Value {
    rt.post_json(&format!("/api/v1/apps/{app}/deletion"), &json!({}))
        .await
        .1
}

fn confirmation(operation: &Value) -> String {
    format!(
        "/api/v1/app-deletions/{}/confirm",
        operation["id"].as_str().unwrap()
    )
}

async fn await_state(pool: &PgPool, id: &str, state: &str) {
    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            let current: String = sqlx::query_scalar(
                "SELECT state FROM rootcx_system.app_deletions WHERE id=$1::uuid",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
            if current == state {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("operation {id} did not reach {state}"));
}

async fn source_history(rt: &TestRuntime, app: &str) -> Uuid {
    let human: Uuid =
        sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email='admin@test.local'")
            .fetch_one(rt.pool())
            .await
            .unwrap();
    sqlx::query("INSERT INTO rootcx_system.source_projects(app_id,head_commit) VALUES($1,$2)")
        .bind(app)
        .bind("a".repeat(40))
        .execute(rt.pool())
        .await
        .unwrap();
    let conversation = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.source_conversations(id,app_id,user_id,title) VALUES($1,$2,$3,'History')")
        .bind(conversation).bind(app).bind(human).execute(rt.pool()).await.unwrap();
    let run = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.source_runs(id,app_id,requested_by,request_id,base_commit,prompt,status,conversation_id) VALUES($1,$2,$3,$4,$5,'Test','succeeded',$6)")
        .bind(run).bind(app).bind(human).bind(Uuid::new_v4()).bind("a".repeat(40)).bind(conversation).execute(rt.pool()).await.unwrap();
    std::fs::create_dir_all(rt.runtime.data_dir().join("sources").join(app)).unwrap();
    std::fs::write(
        rt.runtime
            .data_dir()
            .join("sources")
            .join(app)
            .join("history"),
        "source history",
    )
    .unwrap();
    run
}

#[tokio::test]
async fn deletion_permission_is_native_and_revalidated_after_confirmation_wait() {
    let rt = TestRuntime::boot().await;
    rt.install("permission_test", "contacts").await;
    let token = rt.create_user("delete-user@test.local").await;
    let path = "/api/v1/apps/permission_test/deletion";
    assert_eq!(
        rt.request_as(reqwest::Method::POST, path, &token, Some(&json!({})))
            .await
            .0,
        403
    );
    sqlx::query("INSERT INTO rootcx_system.rbac_roles(name,permissions) VALUES('deletion-test',ARRAY['admin:apps.delete'])")
        .execute(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments(user_id,role) SELECT id,'deletion-test' FROM rootcx_system.users WHERE email='delete-user@test.local'")
        .execute(rt.pool()).await.unwrap();
    let (status, operation) = rt
        .request_as(reqwest::Method::POST, path, &token, Some(&json!({})))
        .await;
    assert_eq!(
        status, 200,
        "explicit deletion permission is sufficient: {operation}"
    );
    assert_eq!(
        rt.post_json(&confirmation(&operation), &json!({})).await.0,
        403,
        "even an admin cannot consume another person's confirmation"
    );
    sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions='{}' WHERE name='deletion-test'")
        .execute(rt.pool())
        .await
        .unwrap();
    assert_eq!(
        rt.request_as(
            reqwest::Method::POST,
            &confirmation(&operation),
            &token,
            Some(&json!({}))
        )
        .await
        .0,
        403
    );
    assert_eq!(rt.get_json("/api/v1/apps/permission_test").await.0, 200);
    let refused: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.app_deletion_events WHERE event='confirmation_refused'",
    )
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert_eq!(refused, 2);
    assert_eq!(
        rt.delete("/api/v1/apps/permission_test?deleteSources=true")
            .await,
        409,
        "a boolean flag cannot bypass confirmation"
    );
    assert_eq!(
        rt.delete("/api/v1/integrations/catalog/permission_test")
            .await,
        404,
        "integration API cannot bypass app confirmation"
    );
    let direct = rt
        .post_json(
            "/api/v1/tools/delete_app/execute",
            &json!({"appId":"assistant","args":{"appId":"permission_test"}}),
        )
        .await;
    assert_eq!(
        direct.0, 400,
        "raw HTTP tool execution cannot invent agent authority"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn deletion_confirmation_cannot_outlive_its_expiry_or_installation() {
    let rt = TestRuntime::boot().await;
    rt.install("generation_test", "contacts").await;
    let expired = prepare(&rt, "generation_test").await;
    sqlx::query("UPDATE rootcx_system.app_deletions SET expires_at=now() WHERE id=$1::uuid")
        .bind(expired["id"].as_str().unwrap())
        .execute(rt.pool())
        .await
        .unwrap();
    assert_eq!(
        rt.post_json(&confirmation(&expired), &json!({})).await.0,
        409
    );
    let stale = prepare(&rt, "generation_test").await;
    let replacement = prepare(&rt, "generation_test").await;
    assert_eq!(
        rt.post_json(&confirmation(&replacement), &json!({}))
            .await
            .0,
        200
    );
    await_state(rt.pool(), replacement["id"].as_str().unwrap(), "succeeded").await;
    rt.install("generation_test", "contacts").await;
    assert_eq!(rt.post_json(&confirmation(&stale), &json!({})).await.0, 409);
    assert_eq!(rt.get_json("/api/v1/apps/generation_test").await.0, 200);
    for app in ["public", "pg_catalog", "assistant", "missing", "bad%2Fpath"] {
        assert!(
            rt.post_json(&format!("/api/v1/apps/{app}/deletion"), &json!({}))
                .await
                .0
                .is_client_error(),
            "{app}"
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn deletion_refuses_every_unfinished_builder_state_without_revoking_the_app() {
    let rt = TestRuntime::boot().await;
    rt.install("busy_test", "contacts").await;
    let run = source_history(&rt, "busy_test").await;
    let operation = prepare(&rt, "busy_test").await;
    for state in ["queued", "coding", "publishing", "needs_recovery"] {
        sqlx::query("UPDATE rootcx_system.source_runs SET status=$1 WHERE id=$2")
            .bind(state)
            .bind(run)
            .execute(rt.pool())
            .await
            .unwrap();
        assert_eq!(
            rt.post_json(&confirmation(&operation), &json!({})).await.0,
            409,
            "{state}"
        );
        let active: bool = sqlx::query_scalar(
            "SELECT active FROM rootcx_system.app_installations WHERE id=$1::uuid",
        )
        .bind(operation["installationId"].as_str().unwrap())
        .fetch_one(rt.pool())
        .await
        .unwrap();
        assert!(active, "{state}");
        assert!(
            rt.runtime
                .data_dir()
                .join("sources/busy_test/history")
                .exists()
        );
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn deletion_is_idempotent_audited_and_does_not_touch_other_apps() {
    let rt = TestRuntime::boot().await;
    rt.install("remove_me", "contacts").await;
    rt.install("keep_me", "contacts").await;
    source_history(&rt, "remove_me").await;
    let oid: i64 = sqlx::query_scalar("SELECT lo_create(0)::bigint")
        .fetch_one(rt.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO rootcx_system.files(app_id,name,size,content_oid) VALUES('remove_me','stored file',0,$1::bigint::oid)")
        .bind(oid).execute(rt.pool()).await.unwrap();
    let operation = prepare(&rt, "remove_me").await;
    let url = confirmation(&operation);
    let body = json!({});
    let (first, second) = tokio::join!(rt.post_json(&url, &body), rt.post_json(&url, &body));
    assert_eq!(first.0, 200);
    assert_eq!(second.0, 200);
    assert_eq!(first.1["id"], second.1["id"]);
    let id = operation["id"].as_str().unwrap();
    await_state(rt.pool(), id, "succeeded").await;
    for table in [
        "apps",
        "source_projects",
        "source_runs",
        "source_conversations",
    ] {
        let key = if table == "apps" { "id" } else { "app_id" };
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM rootcx_system.{table} WHERE {key}='remove_me'"
        ))
        .fetch_one(rt.pool())
        .await
        .unwrap();
        assert_eq!(count, 0, "{table}");
    }
    assert!(!rt.runtime.data_dir().join("sources/remove_me").exists());
    let retained: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_largeobject_metadata WHERE oid=$1::bigint::oid)",
    )
    .bind(oid)
    .fetch_one(rt.pool())
    .await
    .unwrap();
    assert!(!retained, "stored file large objects must be removed too");
    assert!(
        sqlx::query(
            "INSERT INTO rootcx_system.files(app_id,name,size) VALUES('remove_me','late upload',0)"
        )
        .execute(rt.pool())
        .await
        .is_err(),
        "an upload admitted before deletion cannot publish afterward"
    );
    assert_eq!(rt.get_json("/api/v1/apps/keep_me").await.0, 200);
    let events: Vec<String> = sqlx::query_scalar("SELECT event FROM rootcx_system.app_deletion_events WHERE operation_id=$1::uuid ORDER BY id")
        .bind(id).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(
        events.iter().filter(|event| *event == "confirmed").count(),
        1
    );
    assert_eq!(events.first().unwrap(), "requested");
    assert_eq!(events.last().unwrap(), "succeeded");
    rt.install("remove_me", "contacts").await;
    assert!(sqlx::query("INSERT INTO rootcx_system.files(app_id,name,size) VALUES('remove_me','new installation upload',0)").execute(rt.pool()).await.is_ok());
    assert_eq!(
        rt.post_json(&confirmation(&operation), &json!({})).await.1["state"],
        "succeeded"
    );
    assert_eq!(
        rt.get_json("/api/v1/apps/remove_me").await.0,
        200,
        "replaying a completed operation cannot delete a reinstall"
    );
    rt.shutdown().await;
}

#[tokio::test]
async fn deletion_recovers_after_filesystem_failure_and_core_restart() {
    let rt = TestRuntime::boot().await;
    rt.install("restart_test", "contacts").await;
    source_history(&rt, "restart_test").await;
    let root = rt.runtime.data_dir().to_path_buf();
    std::fs::create_dir_all(root.join("frontends")).unwrap();
    std::fs::write(
        root.join("frontends/restart_test"),
        "blocks directory cleanup",
    )
    .unwrap();
    let operation = prepare(&rt, "restart_test").await;
    assert_eq!(
        rt.post_json(&confirmation(&operation), &json!({})).await.0,
        200
    );
    let id = operation["id"].as_str().unwrap();
    await_state(rt.pool(), id, "retry").await;
    assert!(
        root.join("sources/restart_test/history").exists(),
        "history remains until final cleanup"
    );
    for path in [
        "/api/v1/apps/restart_test/collections/contacts",
        "/apps/restart_test/",
        "/api/v1/apps/restart_test/sources",
    ] {
        assert_eq!(rt.get_json(path).await.0, 409, "{path}");
    }
    assert_eq!(
        rt.post_json(
            "/api/v1/apps",
            &json!({"appId":"restart_test","name":"Reinstall","version":"1.0.0","dataContract":[]})
        )
        .await
        .0,
        409
    );
    let bun = rt.runtime.bun_bin().to_path_buf();
    let resources = rt.runtime.resources_dir().to_path_buf();
    rt.runtime.shutdown().await;
    std::fs::remove_file(root.join("frontends/restart_test")).unwrap();
    let restarted = Arc::new(
        rootcx_core::Runtime::new(
            std::env::var("TEST_DATABASE_URL").unwrap(),
            root.clone(),
            resources,
            bun,
        )
        .with_assistant_dir(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assistant"),
        )
        .boot(0)
        .await
        .unwrap(),
    );
    rootcx_core::governance::app_deletion::spawn(restarted.clone());
    await_state(restarted.pool(), id, "succeeded").await;
    assert!(!root.join("sources/restart_test").exists());
    let attempts: i32 =
        sqlx::query_scalar("SELECT attempts FROM rootcx_system.app_deletions WHERE id=$1::uuid")
            .bind(id)
            .fetch_one(restarted.pool())
            .await
            .unwrap();
    assert!(attempts >= 2);
    let confirmations: i64 = sqlx::query_scalar("SELECT count(*) FROM rootcx_system.app_deletion_events WHERE operation_id=$1::uuid AND event='confirmed'")
        .bind(id).fetch_one(restarted.pool()).await.unwrap();
    assert_eq!(
        confirmations, 1,
        "restart must not require another confirmation"
    );
    restarted.shutdown().await;
    rt.shutdown().await;
}
