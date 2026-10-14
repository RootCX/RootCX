mod harness;
use axum::{Json, Router, response::IntoResponse, routing::post};
use base64::{Engine, engine::general_purpose::STANDARD};
use harness::TestRuntime;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[test]
fn core_validation_repairs_candidates_before_publication_and_stops_at_the_limit() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    unsafe {
        std::env::set_var("ROOTCX_BUILDER_URL", format!("http://{addr}"));
        std::env::set_var(
            "ROOTCX_BUILDER_TOKEN",
            "builder-repair-test-token-000000000000",
        );
        std::env::set_var("ROOTCX_BUILDER_ALLOW_UNBACKED_SOURCES", "true");
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured = requests.clone();
        let runner = tokio::spawn(async move {
            axum::serve(tokio::net::TcpListener::from_std(listener).unwrap(), Router::new().route("/build", post(move |Json(mut input): Json<Value>| {
                let captured = captured.clone();
                async move {
                    let mut requests = captured.lock().unwrap();
                    let attempt = requests.iter().filter(|request| request["runId"] == input["runId"]).count();
                    requests.push(input.clone());
                    if input["appId"] == "repair_unavailable" {
                        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error":"AI_UNAVAILABLE"}))).into_response();
                    }
                    if input["appId"] == "repair_unsafe" {
                        input["files"]["../escape"] = json!(STANDARD.encode("unsafe"));
                        return Json(json!({"files":input["files"],"frontend":"","summary":"Must fail"})).into_response();
                    }
                    let mut manifest: Value = serde_json::from_slice(&STANDARD.decode(input["files"]["manifest.json"].as_str().unwrap()).unwrap()).unwrap();
                    let entity = &mut manifest["dataContract"][0];
                    if attempt == 0 || input["appId"] == "repair_exhausted" {
                        if input["appId"] == "repair_type" {
                            entity["fields"][0]["type"] = json!("invented_type");
                        } else {
                            entity.as_object_mut().unwrap().remove("entityName");
                            entity["entity"] = json!("customers");
                        }
                    } else {
                        entity.as_object_mut().unwrap().remove("entity");
                        entity["entityName"] = json!("customers");
                        entity["fields"][0]["type"] = json!("text");
                        entity["fields"].as_array_mut().unwrap().push(json!({"name":"notes","type":"text"}));
                    }
                    input["files"]["manifest.json"] = json!(STANDARD.encode(serde_json::to_vec(&manifest).unwrap()));
                    input["files"]["src/App.tsx"] = json!(STANDARD.encode("candidate to preserve during repair"));
                    let frontend = harness::make_tar_gz(&[("index.html", b"<html>Repaired customer app</html>")]);
                    Json(json!({"files":input["files"],"frontend":STANDARD.encode(frontend),"summary":"Votre application est prête."})).into_response()
                }
            }))).await.unwrap();
        });
        let rt = TestRuntime::boot().await;
        for (app, attempts, status, diagnostic) in [
            ("repair_entity", 2, "succeeded", "entityName"),
            ("repair_type", 2, "succeeded", "invented_type"),
            ("repair_exhausted", 3, "failed", "entityName"),
            ("repair_unavailable", 1, "failed", ""),
            ("repair_unsafe", 1, "failed", ""),
        ] {
            let manifest = json!({"appId":app,"name":"Repair test","version":"1.0.0","dataContract":[{"entityName":"customers","fields":[{"name":"name","type":"text"}]}]});
            assert_eq!(rt.post_json("/api/v1/apps", &manifest).await.0, 200, "{app}");
            let (_, customer) = rt.post_json(&format!("/api/v1/apps/{app}/collections/customers"), &json!({"name":"Existing customer"})).await;
            let files = json!({"manifest.json":STANDARD.encode(serde_json::to_vec(&manifest).unwrap()),"src/App.tsx":STANDARD.encode("published source")});
            let (_, before) = rt.post_json(&format!("/api/v1/apps/{app}/sources"), &json!({"files":files})).await;
            let request = json!({"requestId":uuid::Uuid::new_v4(),"conversationId":uuid::Uuid::new_v4(),"baseCommit":before["headCommit"],"prompt":"Add optional customer notes"});
            let (accepted, run) = rt.post_json(&format!("/api/v1/apps/{app}/changes"), &request).await;
            assert_eq!(accepted, 200, "{app}: {run}");
            let completed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                loop {
                    let (_, result) = rt.get_json(&format!("/api/v1/apps/{app}/changes/{}",run["id"].as_str().unwrap())).await;
                    if ["succeeded","failed","needs_recovery"].contains(&result["status"].as_str().unwrap()) { break result; }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }).await.unwrap();
            assert_eq!(completed["status"], status, "{app}: {completed}");
            let calls: Vec<_> = requests.lock().unwrap().iter().filter(|input| input["appId"] == app).cloned().collect();
            assert_eq!(calls.len(), attempts, "{app}: bounded repair attempts");
            for call in &calls {
                assert_eq!(call["narrationPrompt"], request["prompt"], "{app}: narration must never receive repair diagnostics");
            }
            assert!(calls[0]["conversationId"].is_string(), "{app}: conversation must be retained");
            for call in &calls[1..] {
                assert_eq!(call["runId"], calls[0]["runId"], "{app}");
                assert_eq!(call["conversationId"], calls[0]["conversationId"], "{app}");
                assert_eq!(call["baseCommit"], before["headCommit"], "{app}");
                assert!(call["prompt"].as_str().unwrap().contains(diagnostic), "{app}: Core diagnostic missing");
                assert!(call["prompt"].as_str().unwrap().contains("Add optional customer notes"), "{app}: original intent missing");
                assert_eq!(STANDARD.decode(call["files"]["src/App.tsx"].as_str().unwrap()).unwrap(), b"candidate to preserve during repair", "{app}: rejected candidate must be retained");
            }
            assert!(!completed["message"].as_str().unwrap().contains("entityName"), "{app}: internal diagnostic must not appear in business message");
            let (_, after) = rt.get_json(&format!("/api/v1/apps/{app}/sources")).await;
            let (_, records) = rt.get_json(&format!("/api/v1/apps/{app}/collections/customers")).await;
            assert_eq!(records.as_array().unwrap().len(), 1, "{app}");
            assert_eq!(records[0]["id"], customer["id"], "{app}: existing record ID");
            assert_eq!(records[0]["name"], "Existing customer", "{app}");
            if status == "succeeded" {
                assert_ne!(after["headCommit"], before["headCommit"], "{app}");
                assert_eq!(after["deployedCommit"], completed["commitId"], "{app}");
                let html = rt.client.get(rt.url(&format!("/apps/{app}/"))).send().await.unwrap().text().await.unwrap();
                assert!(html.contains("Repaired customer app"), "{app}");
                assert_eq!(rt.post_json(&format!("/api/v1/apps/{app}/collections/customers"), &json!({"name":"After repair","notes":"Works"})).await.0, 201, "{app}: repaired schema must support writes");
            } else {
                assert_eq!(after, before, "{app}: failed candidates must not change published revision");
                assert!(completed["commitId"].is_null(), "{app}");
            }
        }
        runner.abort();
    });
}
