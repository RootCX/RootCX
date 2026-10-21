mod harness;
use axum::{Json, Router, routing::post};
use base64::{Engine, engine::general_purpose::STANDARD};
use harness::TestRuntime;
use serde_json::{Value, json};

fn encode(value: &Value) -> String {
    STANDARD.encode(serde_json::to_vec(value).unwrap())
}

fn release(version: &str, description: &str, app: &str, readme: &str) -> Value {
    let manifest = json!({"appId":"upgrade_test","name":"Upgrade Test","version":version,
        "dataContract":[{"entityName":"items","fields":[{"name":"label","type":"text"}]}],
        "actions":[{"id":"create_item","name":"Create item","description":description}]});
    json!({"manifest.json":encode(&manifest),"src/App.tsx":STANDARD.encode(app),"src/Readme.tsx":STANDARD.encode(readme)})
}

fn frontend(body: &str) -> String {
    STANDARD.encode(harness::make_tar_gz(&[(
        "index.html",
        format!("<html><body>{body}</body></html>").as_bytes(),
    )]))
}

#[test]
fn release_upgrade_merges_application_changes_without_a_model() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    unsafe {
        std::env::set_var("ROOTCX_BUILDER_URL", format!("http://{addr}"));
        std::env::set_var(
            "ROOTCX_BUILDER_TOKEN",
            "builder-upgrade-token-0000000000000000",
        );
        std::env::set_var("ROOTCX_BUILDER_ALLOW_UNBACKED_SOURCES", "true");
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async move {
        // The application's own change: a UI edit published through the Builder.
        let runner = tokio::spawn(async move {
            axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), Router::new().route("/build", post(|Json(mut input): Json<Value>| async move {
                input["files"]["src/App.tsx"] = json!(STANDARD.encode("export const sidebar = 'fixed';"));
                Json(json!({"files":input["files"],"frontend":frontend("fixed sidebar"),"summary":"Sidebar fixed."}))
            }))).await.unwrap();
        });
        let rt = TestRuntime::boot().await;
        let v1 = release("1.0.0", "", "export const sidebar = 'broken';", "v1");
        let manifest: Value = serde_json::from_slice(&STANDARD.decode(v1["manifest.json"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(rt.post_json("/api/v1/apps", &manifest).await.0, 200);
        let (status, imported) = rt.post_json("/api/v1/apps/upgrade_test/sources", &json!({"files":v1})).await;
        assert_eq!(status, 200, "{imported}");
        let (_, change) = rt.post_json("/api/v1/apps/upgrade_test/changes", &json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":imported["headCommit"],"prompt":"Fix the sidebar"})).await;
        let changed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let (_, run) = rt.get_json(&format!("/api/v1/apps/upgrade_test/changes/{}", change["id"].as_str().unwrap())).await;
                if run["status"] == "succeeded" { break run; }
                assert_ne!(run["status"], "failed", "{run}");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        let head = changed["commitId"].as_str().unwrap().to_string();

        // Release 2.0.0 documents an action and edits a non-UI file only.
        let v2 = release("2.0.0", "Creates an item.", "export const sidebar = 'broken';", "v1");
        let request = json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":head,"files":v2,"frontend":frontend("release 2 ui")});
        let (status, run) = rt.post_json("/api/v1/apps/upgrade_test/sources/upgrade", &request).await;
        assert_eq!(status, 200, "{run}");
        assert_eq!(run["status"], "succeeded", "{run}");
        assert_eq!(rt.post_json("/api/v1/apps/upgrade_test/sources/upgrade", &request).await.1["id"], run["id"], "a retried request must not upgrade twice");
        let (_, sources) = rt.get_json("/api/v1/apps/upgrade_test/sources").await;
        assert_eq!(sources["headCommit"], run["commitId"]);
        assert_eq!(sources["deployedCommit"], run["commitId"]);
        let installed: Value = sqlx::query_scalar("SELECT manifest FROM rootcx_system.apps WHERE id='upgrade_test'").fetch_one(rt.pool()).await.unwrap();
        assert_eq!(installed["version"], "2.0.0");
        assert_eq!(installed["actions"][0]["description"], "Creates an item.");
        let html = rt.client.get(rt.url("/apps/upgrade_test/")).send().await.unwrap().text().await.unwrap();
        assert!(html.contains("fixed sidebar"), "the application's own UI must stay published: {html}");
        let upgraded = run["commitId"].as_str().unwrap().to_string();

        let other = rt.create_user("upgrade-reader@test.local").await;
        assert_eq!(rt.request_as(reqwest::Method::POST, "/api/v1/apps/upgrade_test/sources/upgrade", &other,
            Some(&json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":upgraded,"files":release("3.0.0", "x", "export const sidebar = 'broken';", "v1"),"frontend":frontend("x")}))).await.0, 403);
        for (case, base, files, needle) in [
            ("stale revision", head.as_str(), release("3.0.0", "Creates an item.", "export const sidebar = 'broken';", "v3"), "version changed"),
            ("already applied", upgraded.as_str(), v2.clone(), "already applied"),
            ("conflict", upgraded.as_str(), release("3.0.0", "Creates an item.", "export const sidebar = 'collapsible';", "v1"), "conflicts"),
            ("rebuild", upgraded.as_str(), release("3.0.0", "Creates an item.", "export const sidebar = 'broken';", "v3"), "rebuild"),
        ] {
            let (status, body) = rt.post_json("/api/v1/apps/upgrade_test/sources/upgrade",
                &json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":base,"files":files,"frontend":frontend("release 3 ui")})).await;
            assert_eq!(status, 409, "{case}: {body}");
            assert!(body.to_string().contains(needle), "{case}: {body}");
            let (_, unchanged) = rt.get_json("/api/v1/apps/upgrade_test/sources").await;
            assert_eq!(unchanged["headCommit"], upgraded.as_str(), "{case} must leave the revision untouched");
        }
        let still = rt.client.get(rt.url("/apps/upgrade_test/")).send().await.unwrap().text().await.unwrap();
        assert_eq!(still, html, "refused upgrades must keep the published UI");
        runner.abort();
        rt.shutdown().await;
    });
}
