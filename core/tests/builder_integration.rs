mod harness;
use axum::{Json, Router, routing::post};
use base64::{Engine, engine::general_purpose::STANDARD};
use harness::TestRuntime;
use serde_json::{Value, json};

#[test]
fn source_to_git_to_schema_to_published_frontend() {
    // Set process configuration before creating any async runtime threads.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    unsafe {
        std::env::set_var("ROOTCX_BUILDER_URL", format!("http://{addr}"));
        std::env::set_var(
            "ROOTCX_BUILDER_TOKEN",
            "builder-integration-token-000000000000",
        );
        std::env::set_var("ROOTCX_BUILDER_ALLOW_UNBACKED_SOURCES", "true");
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let runner=tokio::spawn(async move {
            axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(),Router::new().route("/build",post(|Json(mut input):Json<Value>|async move {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                if input["prompt"] == "Invalid build fixture" {
                    input["files"]["src/App.tsx"] = json!(STANDARD.encode("invalid build"));
                    return Json(json!({"files":input["files"],"frontend":"broken","summary":"Should never publish"}));
                }
                let mut manifest:Value=serde_json::from_slice(&STANDARD.decode(input["files"]["manifest.json"].as_str().unwrap()).unwrap()).unwrap();
                manifest["dataContract"][0]["fields"].as_array_mut().unwrap().push(json!({"name":"visit_date","type":"date"}));
                input["files"]["manifest.json"]=json!(STANDARD.encode(serde_json::to_vec(&manifest).unwrap()));
                input["files"]["src/App.tsx"]=json!(STANDARD.encode("export const visitDate = true;"));
                let frontend=harness::make_tar_gz(&[("index.html",b"<html><body><input name='visit_date' type='date'></body></html>")]);
                Json(json!({"files":input["files"],"frontend":STANDARD.encode(frontend),"summary":"La date de visite est disponible."}))
            }))).await.unwrap();
        });
        let rt=TestRuntime::boot().await;
        let manifest=json!({"appId":"builder_test","name":"Builder Test","version":"1.0.0","dataContract":[{"entityName":"quotes","fields":[{"name":"label","type":"text"}]}]});
        assert_eq!(rt.post_json("/api/v1/apps",&manifest).await.0,200);
        let files=json!({"manifest.json":STANDARD.encode(serde_json::to_vec(&manifest).unwrap()),"src/App.tsx":STANDARD.encode("export const visitDate = false;")});
        let (status,imported)=rt.post_json("/api/v1/apps/builder_test/sources",&json!({"files":files})).await;
        assert_eq!(status,200,"{imported}");
        let base=imported["headCommit"].as_str().unwrap();
        assert_eq!(rt.post_json("/api/v1/apps/builder_test/sources",&json!({"files":files})).await.0,409,"re-import must never overwrite history");
        let other=rt.create_user("builder-reader@test.local").await;
        assert_eq!(rt.request_as(reqwest::Method::GET,"/api/v1/apps/builder_test/sources",&other,None).await.0,403);
        let request=json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":base,"prompt":"Ajoute une date de visite."});
        let (status,run)=rt.post_json("/api/v1/apps/builder_test/changes",&request).await;
        assert_eq!(status,200,"{run}");
        let retry=rt.post_json("/api/v1/apps/builder_test/changes",&request).await;
        assert_eq!(retry.1["id"],run["id"],"network retry must reuse the same run");
        let concurrent=json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":base,"prompt":"Une autre demande."});
        assert_eq!(rt.post_json("/api/v1/apps/builder_test/changes",&concurrent).await.0,409);
        let path=format!("/api/v1/apps/builder_test/changes/{}",run["id"].as_str().unwrap());
        let final_run=tokio::time::timeout(std::time::Duration::from_secs(30),async{
            loop {
                let (_,run)=rt.get_json(&path).await;
                if !["queued","coding","publishing"].contains(&run["status"].as_str().unwrap()){break run;}
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        assert_eq!(final_run["status"],"succeeded","{final_run}");
        let (status,record)=rt.post_json("/api/v1/apps/builder_test/collections/quotes",&json!({"label":"Visit","visit_date":"2026-10-10"})).await;
        assert_eq!(status,201,"{record}");
        assert_eq!(record["visit_date"],"2026-10-10");
        let html=rt.client.get(rt.url("/apps/builder_test/")).send().await.unwrap().text().await.unwrap();
        assert!(html.contains("visit_date"),"new UI must be served after the column exists");
        let (_,source)=rt.get_json("/api/v1/apps/builder_test/sources").await;
        assert_eq!(source["headCommit"],final_run["commitId"]);
        assert_eq!(source["headCommit"],source["deployedCommit"]);
        assert_ne!(source["headCommit"],base);
        assert_eq!(rt.post_json("/api/v1/apps",&manifest).await.0,409,"external deployments must not invalidate managed source history");
        // Crash injection: files and Git activated, SQL completion not yet committed.
        sqlx::query("UPDATE rootcx_system.source_projects SET head_commit=$1,deployed_commit=NULL WHERE app_id='builder_test'").bind(base).execute(rt.pool()).await.unwrap();
        sqlx::query("UPDATE rootcx_system.source_runs SET status='needs_recovery' WHERE id=$1::uuid").bind(run["id"].as_str().unwrap()).execute(rt.pool()).await.unwrap();
        assert_eq!(rt.post_json("/api/v1/apps/builder_test/changes",&concurrent).await.0,409,"recovery must block new changes");
        assert_eq!(rt.post_json(&format!("{path}/retry-publication"),&json!({})).await.0,200);
        let recovered=tokio::time::timeout(std::time::Duration::from_secs(15),async {
            loop {
                let (_,state)=rt.get_json(&path).await;
                if state["status"]!="publishing" {break state;}
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        assert_eq!(recovered["status"],"succeeded","{recovered}");
        assert_eq!(recovered["commitId"],final_run["commitId"],"recovery must activate the same commit");
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM builder_test.quotes").fetch_one(rt.pool()).await.unwrap();
        assert_eq!(count,1,"recovery must preserve existing business data");
        let failed_request=json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":final_run["commitId"],"prompt":"Invalid build fixture"});
        let (status,failed)=rt.post_json("/api/v1/apps/builder_test/changes",&failed_request).await;
        assert_eq!(status,200,"{failed}");
        let failed_path=format!("/api/v1/apps/builder_test/changes/{}",failed["id"].as_str().unwrap());
        let failed_state=tokio::time::timeout(std::time::Duration::from_secs(15),async {
            loop {
                let (_,state)=rt.get_json(&failed_path).await;
                if !["queued","coding","publishing"].contains(&state["status"].as_str().unwrap()) {break state;}
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        assert_eq!(failed_state["status"],"failed","{failed_state}");
        let (_,unchanged)=rt.get_json("/api/v1/apps/builder_test/sources").await;
        assert_eq!(unchanged["headCommit"],final_run["commitId"]);
        assert_eq!(unchanged["deployedCommit"],final_run["commitId"]);
        let still_served=rt.client.get(rt.url("/apps/builder_test/")).send().await.unwrap().text().await.unwrap();
        assert_eq!(still_served,html,"invalid build must preserve published UI");
        let bundle=rt.client.get(rt.url("/api/v1/apps/builder_test/sources/bundle")).bearer_auth(&rt.token).send().await.unwrap();
        assert_eq!(bundle.status(),200);
        assert!(bundle.bytes().await.unwrap().starts_with(b"# v2 git bundle"));
        runner.abort();rt.shutdown().await;
    });
}
