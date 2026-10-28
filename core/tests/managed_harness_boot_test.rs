mod harness;

use axum::{Json, Router, routing::post, response::IntoResponse};
use serde_json::{json, Value};
use reqwest::StatusCode;
use rootcx_core::RpcCaller;
use std::{path::Path, time::Duration};
use uuid::Uuid;

// Real Deep Agents + Core + Bun backend + PostgreSQL. Only model responses are
// scripted: this tests runtime selection, not the model's ability to pick tools.
#[tokio::test]
async fn managed_agents_preserve_mixed_app_actions_jobs_and_lifecycle() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let bundle = root.join("runtime/agent-harness/dist/index.js");
    assert!(bundle.is_file(), "build runtime/agent-harness before this integration test");
    unsafe { std::env::set_var("ROOTCX_AGENT_HARNESS", &bundle); }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let model = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/messages", post(|Json(body): Json<Value>| async move {
            assert!(body["messages"].to_string().contains("PINNED_SKILL_BODY_FROM_DEPLOYMENT"), "The model must receive the configured skill body, not only its metadata");
            let app = if body.to_string().contains("inventory_fixture") { "inventory_fixture" } else { "support_fixture" };
            let tool_result = body["messages"].as_array().unwrap().last().unwrap()["content"].as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"));
            let content = if tool_result { json!([{"type":"text","text":"Recorded."}]) }
                else { json!([{"type":"tool_use","id":"tool-fixture","name":"call_action","input":{"app":app,"action":"record","input":{"name":"agent"}}}]) };
            let reason = if tool_result {"end_turn"} else {"tool_use"};
            let response = json!({"id":Uuid::new_v4().to_string(),"type":"message","role":"assistant","model":"fixture","content":content,
                "stop_reason":reason,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":5}});
            if body["stream"] == true {
                let mut start = response.clone(); start["content"] = json!([]); start["stop_reason"] = Value::Null;
                let block = &content[0];
                let (initial, delta) = if tool_result {
                    (json!({"type":"text","text":""}), json!({"type":"text_delta","text":"Recorded."}))
                } else {
                    (json!({"type":"tool_use","id":"tool-fixture","name":"call_action","input":{}}),
                     json!({"type":"input_json_delta","partial_json":block["input"].to_string()}))
                };
                let frames = [json!({"type":"message_start","message":start}),
                    json!({"type":"content_block_start","index":0,"content_block":initial}),
                    json!({"type":"content_block_delta","index":0,"delta":delta}),
                    json!({"type":"content_block_stop","index":0}),
                    json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":{"output_tokens":5}}),
                    json!({"type":"message_stop"})];
                return ([("content-type", "text/event-stream")], frames.iter().map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap())).collect::<String>()).into_response();
            }
            Json(response).into_response()
        }))).await.unwrap();
    });
    let rt = harness::TestRuntime::boot().await;
    let (status, body) = rt.post_json("/api/v1/llm-models", &json!({"id":"fixture","name":"Local fixture","provider":"rootcx","model":"fixture","config":{},"is_default":true})).await;
    assert!(status.is_success(), "{body}");
    let uid: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email='admin@test.local'").fetch_one(rt.pool()).await.unwrap();
    for app in ["inventory_fixture", "support_fixture"] {
        let manifest = json!({"appId":app,"name":app,"version":"1.0.0",
            "instructions":format!("Manage records in {app} using record. No other app is relevant."),
            "actions":[{"id":"record","name":"Record","inputSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}],
            "dataContract":[{"entityName":"records","fields":[{"name":"name","type":"text"}]}]});
        rt.install_manifest(&manifest).await;
        let config = json!({"name":app,"harness":{"type":"deepagents"},"systemPrompt":"agent/system.md",
            "pinnedSkills":["record"],
            "memory":{"enabled":true},"limits":{"maxTurns":2},"supervision":{"mode":"autonomous"}});
        let backend = br#"serve({
            onStart: async () => { await Bun.write('lifecycle.marker', 'started'); },
            rpc: { record: (p, _caller, ctx) => ctx.collection('records').insert({name:p.name}) },
            onJob: (p, _caller, ctx) => ctx.collection('records').insert({name:p.name}),
        });"#;
        let prompt = format!("Use the {app} app to record the requested fact.");
        let (status, body) = rt.deploy(app, &harness::make_tar_gz(&[
            ("manifest.json",manifest.to_string().as_bytes()), ("agent.json",config.to_string().as_bytes()),
            ("agent/system.md",prompt.as_bytes()),("index.ts",backend),
            ("agent/skills/record/SKILL.md", b"---\nname: record\ndescription: Record facts\n---\nPINNED_SKILL_BODY_FROM_DEPLOYMENT"),
        ])).await;
        assert_eq!(status, StatusCode::OK, "{app}: {body}");
        for (key, value) in [("ROOTCX_API_KEY", "local-only"), ("ROOTCX_LLM_ENDPOINT", endpoint.as_str())] {
            let (status, body) = rt.post_json(&format!("/api/v1/apps/{app}/secrets"), &json!({"key":key,"value":value})).await;
            assert!(status.is_success(), "{body}");
        }
        sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions=array_append(permissions, 'tool:call_action') WHERE name=$1")
            .bind(format!("app:{app}:agent")).execute(rt.pool()).await.unwrap();
        let caller = || RpcCaller {user_id:uid.to_string(),email:"admin@test.local".into(),effective_perms:None,connection_id:None};
        let rpc = rt.runtime.worker_manager().rpc(app, Uuid::new_v4().to_string(), "record".into(), json!({"name":"human"}), Some(caller())).await.unwrap();
        assert_eq!(rpc["name"], "human", "{app}: ordinary RPC must retain its backend");
        rt.runtime.worker_manager().dispatch_job(app, Uuid::new_v4().to_string(), json!({"name":"job"}), Some(caller()), None).await.unwrap();
        let response = rt.client.post(rt.url(&format!("/api/v1/apps/{app}/agent/invoke")))
            .bearer_auth(&rt.token).json(&json!({"message":"Record a fact"})).timeout(Duration::from_secs(30)).send().await.unwrap();
        let events = response.text().await.unwrap();
        assert!(events.contains("event: done") && events.contains("Recorded."), "{app}: {events}");
        let names: Vec<String> = sqlx::query_scalar(&format!("SELECT name FROM {app}.records ORDER BY name")).fetch_all(rt.pool()).await.unwrap();
        assert_eq!(names, ["agent", "human", "job"], "{app}: only intended effects");
        assert_eq!(std::fs::read_to_string(rt.runtime.data_dir().join(format!("apps/{app}/lifecycle.marker"))).unwrap(), "started");
    }
    rt.shutdown().await;
    model.abort();
}
