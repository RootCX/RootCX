mod harness;
use axum::{Json, Router, response::IntoResponse, routing::post};
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
                    return Json(json!({"files":input["files"],"frontend":"broken","summary":"Should never publish"})).into_response();
                }
                if input["prompt"] == "Bonjour" {
                    return Json(json!({"files":input["files"],"frontend":"","summary":"Bonjour ! Que souhaitez-vous améliorer ?"})).into_response();
                }
                if input["prompt"] == "Credit failure fixture" {
                    return ([("content-type", "application/x-ndjson")], "{\"type\":\"progress\",\"phase\":\"editing\"}\n{\"type\":\"error\",\"error\":\"AI_CREDITS_EXHAUSTED\"}\n").into_response();
                }
                if input["prompt"] == "Delete visit date" {
                    let mut manifest:Value=serde_json::from_slice(&STANDARD.decode(input["files"]["manifest.json"].as_str().unwrap()).unwrap()).unwrap();
                    manifest["dataContract"][0]["fields"].as_array_mut().unwrap().retain(|field| field["name"] != "visit_date");
                    input["files"]["manifest.json"]=json!(STANDARD.encode(serde_json::to_vec(&manifest).unwrap()));
                    input["files"]["src/App.tsx"]=json!(STANDARD.encode("export const visitDate = false;"));
                    let frontend=harness::make_tar_gz(&[("index.html",b"<html><body>Clients</body></html>")]);
                    let result=json!({"type":"result","files":input["files"],"frontend":STANDARD.encode(frontend),"summary":"Le champ a été supprimé."});
                    return ([("content-type", "application/x-ndjson")], format!("{{\"type\":\"progress\",\"phase\":\"editing\"}}\n{result}\n")).into_response();
                }
                let mut manifest:Value=serde_json::from_slice(&STANDARD.decode(input["files"]["manifest.json"].as_str().unwrap()).unwrap()).unwrap();
                manifest["dataContract"][0]["fields"].as_array_mut().unwrap().push(json!({"name":"visit_date","type":"date"}));
                input["files"]["manifest.json"]=json!(STANDARD.encode(serde_json::to_vec(&manifest).unwrap()));
                input["files"]["src/App.tsx"]=json!(STANDARD.encode("export const visitDate = true;"));
                let frontend=harness::make_tar_gz(&[("index.html",b"<html><body><input name='visit_date' type='date'></body></html>")]);
                Json(json!({"files":input["files"],"frontend":STANDARD.encode(frontend),"summary":"La date de visite est disponible."})).into_response()
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
        for (prompt, expected) in [("Credit failure fixture", "failed"), ("Delete visit date", "succeeded")] {
            let request=json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":final_run["commitId"],"prompt":prompt});
            let (status, change)=rt.post_json("/api/v1/apps/builder_test/changes", &request).await;
            assert_eq!(status,200,"{change}");
            let stream_path=format!("/api/v1/apps/builder_test/changes/{}/events",change["id"].as_str().unwrap());
            assert_eq!(rt.request_as(reqwest::Method::GET,&stream_path,&other,None).await.0,403);
            let response=rt.client.get(rt.url(&stream_path)).bearer_auth(&rt.token).send().await.unwrap();
            assert_eq!(response.status(),200);
            let transcript=tokio::time::timeout(std::time::Duration::from_secs(15),response.text()).await.unwrap().unwrap();
            let states:Vec<Value>=transcript.lines().filter_map(|line| line.strip_prefix("data: ")).map(|line| serde_json::from_str(line).unwrap()).collect();
            let last=states.last().expect("SSE must replay persisted state");
            assert_eq!(last["status"],expected,"{transcript}");
            assert!(!transcript.contains("AI_CREDITS_EXHAUSTED"),"internal diagnostics must not leak");
            if expected=="failed" { assert!(last["message"].as_str().unwrap().contains("crédits")); }
            // Reconnect to a completed run: replay terminal state and close immediately.
            let replay=rt.client.get(rt.url(&stream_path)).bearer_auth(&rt.token).send().await.unwrap().text().await.unwrap();
            assert!(replay.contains(expected));
        }
        let field_exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema='builder_test' AND table_name='quotes' AND column_name='visit_date')").fetch_one(rt.pool()).await.unwrap();
        assert!(!field_exists,"explicit deletion must use normal RootCX schema reconciliation");
        let label:String=sqlx::query_scalar("SELECT label FROM builder_test.quotes LIMIT 1").fetch_one(rt.pool()).await.unwrap();
        assert_eq!(label,"Visit");
        let deleted_html=rt.client.get(rt.url("/apps/builder_test/")).send().await.unwrap().text().await.unwrap();
        assert!(!deleted_html.contains("visit_date"));
        let (_,before_greeting)=rt.get_json("/api/v1/apps/builder_test/sources").await;
        let (_,greeting)=rt.post_json("/api/v1/apps/builder_test/changes",&json!({"requestId":uuid::Uuid::new_v4(),"baseCommit":before_greeting["headCommit"],"prompt":"Bonjour"})).await;
        let transcript=rt.client.get(rt.url(&format!("/api/v1/apps/builder_test/changes/{}/events",greeting["id"].as_str().unwrap()))).bearer_auth(&rt.token).send().await.unwrap().text().await.unwrap();
        assert!(transcript.contains("Bonjour !"),"{transcript}");
        assert!(transcript.contains("\"commitId\":null"),"a conversation must not create a revision");
        let (_,after_greeting)=rt.get_json("/api/v1/apps/builder_test/sources").await;
        assert_eq!(before_greeting["headCommit"],after_greeting["headCommit"]);
        let first_conversation = uuid::Uuid::new_v4();
        let second_conversation = uuid::Uuid::new_v4();
        let first_request = json!({"requestId":uuid::Uuid::new_v4(),"conversationId":first_conversation,"baseCommit":after_greeting["headCommit"],"prompt":"Ajoute une date de visite."});
        let (status, first) = rt.post_json("/api/v1/apps/builder_test/changes", &first_request).await;
        assert_eq!(status, 200, "{first}");
        let second_request = json!({"requestId":uuid::Uuid::new_v4(),"conversationId":second_conversation,"baseCommit":after_greeting["headCommit"],"prompt":"Bonjour"});
        let (status, second) = rt.post_json("/api/v1/apps/builder_test/changes", &second_request).await;
        assert_eq!(status, 200, "a parallel topic must queue: {second}");
        // No browser stream is connected: durable dispatch must still finish both requests.
        let (_, completed_second) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let response = rt.get_json(&format!("/api/v1/apps/builder_test/changes/{}", second["id"].as_str().unwrap())).await;
                if response.1["status"] == "succeeded" { break response; }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }).await.unwrap();
        let (_, completed_first) = rt.get_json(&format!("/api/v1/apps/builder_test/changes/{}", first["id"].as_str().unwrap())).await;
        assert_eq!(completed_second["baseCommit"], completed_first["commitId"], "queued work must use the revision published by the previous request");
        let (_, replay) = rt.post_json("/api/v1/apps/builder_test/changes", &second_request).await;
        assert_eq!(replay["id"], second["id"], "a retry after rebasing must not duplicate work");
        let (_, messages) = rt.get_json(&format!("/api/v1/apps/builder_test/conversations/{first_conversation}")).await;
        assert_eq!(messages.as_array().unwrap().len(), 1);
        assert!(messages[0]["activity"].as_array().unwrap().len() >= 2, "activity must survive reconnects");
        sqlx::query("INSERT INTO rootcx_system.rbac_assignments(user_id,role) SELECT id,'admin' FROM rootcx_system.users WHERE email='builder-reader@test.local' ON CONFLICT DO NOTHING").execute(rt.pool()).await.unwrap();
        for endpoint in [format!("conversations/{first_conversation}"), format!("changes/{}", first["id"].as_str().unwrap()), format!("changes/{}/events", first["id"].as_str().unwrap())] {
            assert_eq!(rt.request_as(reqwest::Method::GET, &format!("/api/v1/apps/builder_test/{endpoint}"), &other, None).await.0, 404, "another administrator must not read private conversation context");
        }
        let (_, private_list) = rt.request_as(reqwest::Method::GET, "/api/v1/apps/builder_test/conversations", &other, None).await;
        assert_eq!(private_list, json!([]));
        runner.abort();rt.shutdown().await;
    });
}
