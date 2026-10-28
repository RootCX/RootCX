mod harness;

use axum::{
    Json, Router,
    response::{IntoResponse, Response},
    routing::post,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::sync::{Notify, Semaphore};
use uuid::Uuid;

// Run the production adapter against Core, HTTP, Bun and PostgreSQL. Script only
// the model, so denials and interruptions cannot depend on an obedient prompt.
struct Fixture {
    rt: harness::TestRuntime,
    model: tokio::task::JoinHandle<()>,
    after_write: Arc<Notify>,
    model_inputs: Arc<std::sync::Mutex<Vec<Value>>>,
}
fn model_response(content: Value, streaming: bool) -> Response {
    let block = &content[0];
    let tool = block["type"] == "tool_use";
    let reason = if tool { "tool_use" } else { "end_turn" };
    let response = json!({"id":Uuid::new_v4().to_string(),"type":"message","role":"assistant","model":"fixture","content":content,
        "stop_reason":reason,"stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":5}});
    if !streaming {
        return Json(response).into_response();
    }
    let mut start = response.clone();
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    let (initial, delta) = if tool {
        (
            json!({"type":"tool_use","id":block["id"],"name":block["name"],"input":{}}),
            json!({"type":"input_json_delta","partial_json":block["input"].to_string()}),
        )
    } else {
        (
            json!({"type":"text","text":""}),
            json!({"type":"text_delta","text":block["text"]}),
        )
    };
    let frames = [
        json!({"type":"message_start","message":start}),
        json!({"type":"content_block_start","index":0,"content_block":initial}),
        json!({"type":"content_block_delta","index":0,"delta":delta}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":reason,"stop_sequence":null},"usage":{"output_tokens":5}}),
        json!({"type":"message_stop"}),
    ];
    (
        [("content-type", "text/event-stream")],
        frames
            .iter()
            .map(|v| format!("event: {}\ndata: {v}\n\n", v["type"].as_str().unwrap()))
            .collect::<String>(),
    )
        .into_response()
}
impl Fixture {
    async fn boot(mode: &str, hold_after_write: bool, lose_action_result: bool) -> Self {
        let bundle = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("runtime/agent-harness/dist/index.js");
        assert!(bundle.is_file());
        unsafe {
            std::env::set_var("ROOTCX_AGENT_HARNESS", bundle);
        }
        let after_write = Arc::new(Notify::new());
        let reached = after_write.clone();
        let once = Arc::new(Semaphore::new(usize::from(hold_after_write)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let effect = after_write.clone();
        let model_inputs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let inputs = model_inputs.clone();
        let model = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/v1/messages", post(move |Json(body): Json<Value>| {
                let reached = reached.clone(); let once = once.clone(); let inputs = inputs.clone();
                async move {
                    inputs.lock().unwrap().push(body["messages"].clone());
                    let messages = body["messages"].as_array().unwrap();
                    let has_result = messages.iter().any(|m| m["content"].as_array().is_some_and(|c| c.iter().any(|p| p["type"] == "tool_result")));
                    if has_result && let Ok(permit) = once.try_acquire() {
                        permit.forget(); reached.notify_one();
                        std::future::pending::<()>().await;
                    }
                    let content = if has_result { json!([{"type":"text","text":"Recovery context received; no automatic replay."}]) }
                        else { json!([{"type":"tool_use","id":Uuid::new_v4().to_string(),"name":"call_action","input":{"app":"recovery_fixture","action":"record","input":{"name":"once"}}}]) };
                    model_response(content, body["stream"] == true)
                }
            })).route("/effect", post(move || { let effect = effect.clone(); async move {
                effect.notify_one(); std::future::pending::<String>().await
            }}))).await.unwrap();
        });
        let rt = harness::TestRuntime::boot().await;
        let (status, body) = rt.post_json("/api/v1/llm-models", &json!({"id":"fixture","name":"Local fixture","provider":"rootcx","model":"fixture","config":{},"is_default":true})).await;
        assert!(status.is_success(), "{body}");
        let manifest = json!({"appId":"recovery_fixture","name":"Recovery fixture","version":"1.0.0",
            "actions":[{"id":"record","name":"Record","inputSchema":{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}}],
            "dataContract":[{"entityName":"records","fields":[{"name":"name","type":"text"}]}]});
        rt.install_manifest(&manifest).await;
        let config = json!({"name":"Recovery fixture","harness":{"type":"deepagents"},"memory":{"enabled":true},"limits":{"maxTurns":3},"supervision":{"mode":mode}});
        let wait = if lose_action_result {
            format!("await fetch('{endpoint}/effect', {{method:'POST'}});")
        } else {
            String::new()
        };
        let backend = format!(
            "serve({{rpc:{{record:async(p,_caller,ctx)=>{{ const result=await ctx.collection('records').insert({{name:p.name}}); {wait} return result; }}}}}});"
        );
        let (status, body) = rt
            .deploy(
                "recovery_fixture",
                &harness::make_tar_gz(&[
                    ("manifest.json", manifest.to_string().as_bytes()),
                    ("agent.json", config.to_string().as_bytes()),
                    ("index.ts", backend.as_bytes()),
                ]),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        for (key, value) in [
            ("ROOTCX_API_KEY", "local-only"),
            ("ROOTCX_LLM_ENDPOINT", endpoint.as_str()),
        ] {
            let (status, body) = rt
                .post_json(
                    "/api/v1/apps/recovery_fixture/secrets",
                    &json!({"key":key,"value":value}),
                )
                .await;
            assert!(status.is_success(), "{body}");
        }
        sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions=array_append(permissions,'tool:call_action') WHERE name='app:recovery_fixture:agent'")
            .execute(rt.pool()).await.unwrap();
        Self {
            rt,
            model,
            after_write,
            model_inputs,
        }
    }
    fn invoke(&self, session: Uuid) -> tokio::task::JoinHandle<String> {
        let client = self.rt.client.clone();
        let token = self.rt.token.clone();
        let url = self.rt.url("/api/v1/apps/recovery_fixture/agent/invoke");
        tokio::spawn(async move {
            let response = client.post(url).bearer_auth(token).json(&json!({"session_id":session,"message":"Continue the requested record operation"})).send().await.unwrap();
            assert!(response.status().is_success(), "{}", response.status());
            response.text().await.unwrap()
        })
    }
    async fn approval(&self) -> String {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let (_, rows) = self
                    .rt
                    .get_json("/api/v1/apps/recovery_fixture/agent/approvals")
                    .await;
                if let Some(id) = rows[0]["approvalId"].as_str() {
                    return id.to_string();
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("managed harness must reach approval")
    }
    async fn records(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM recovery_fixture.records")
            .fetch_one(self.rt.pool())
            .await
            .unwrap()
    }
    async fn finish(self) {
        self.rt.shutdown().await;
        self.model.abort();
    }
}
async fn result(run: tokio::task::JoinHandle<String>) -> String {
    tokio::time::timeout(Duration::from_secs(20), run)
        .await
        .expect("run must terminate")
        .unwrap()
}

#[tokio::test]
async fn managed_confirmation_enforces_owner_rejection_and_live_revocation() {
    let f = Fixture::boot("strict", false, false).await;
    let outsider = f.rt.create_user("outsider@test.local").await;
    for case in ["reject", "approve", "revoke"] {
        let run = f.invoke(Uuid::new_v4());
        let approval = f.approval().await;
        let path = format!("/api/v1/apps/recovery_fixture/agent/approvals/{approval}");
        assert_eq!(
            f.records().await,
            i64::from(case == "revoke"),
            "{case}: no write before approval"
        );
        let (status, _) =
            f.rt.request_as(
                reqwest::Method::POST,
                &path,
                &outsider,
                Some(&json!({"action":"approve"})),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{case}: another user cannot approve"
        );
        if case == "revoke" {
            sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions=array_remove(permissions,'tool:call_action') WHERE name='app:recovery_fixture:agent'")
                .execute(f.rt.pool()).await.unwrap();
        }
        let action = if case == "reject" {
            "reject"
        } else {
            "approve"
        };
        let (status, body) = f.rt.post_json(&path, &json!({"action":action})).await;
        assert_eq!(status, StatusCode::OK, "{case}: {body}");
        let events = result(run).await;
        if case != "revoke" {
            assert!(events.contains("event: done"), "{case}: {events}");
        }
        assert_eq!(
            f.records().await,
            i64::from(case != "reject"),
            "{case}: approval never overrides a revoked grant"
        );
        let (status, _) = f.rt.post_json(&path, &json!({"action":"approve"})).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{case}: confirmation is single-use"
        );
    }
    f.finish().await;
}

#[tokio::test]
async fn managed_disconnect_and_worker_stop_invalidate_pending_approval_and_release_session() {
    let f = Fixture::boot("strict", false, false).await;
    for case in ["disconnect", "worker-stop"] {
        let session = Uuid::new_v4();
        let run = f.invoke(session);
        let approval = f.approval().await;
        let response =
            f.rt.client
                .post(f.rt.url("/api/v1/apps/recovery_fixture/agent/invoke"))
                .bearer_auth(&f.rt.token)
                .json(&json!({"session_id":session,"message":"Concurrent"}))
                .send()
                .await
                .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CONFLICT,
            "{case}: serialize the same conversation"
        );
        if case == "disconnect" {
            run.abort();
            let _ = run.await;
        } else {
            f.rt.runtime
                .worker_manager()
                .stop_app("recovery_fixture")
                .await
                .unwrap();
            let _ = result(run).await;
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (_, rows) =
                    f.rt.get_json("/api/v1/apps/recovery_fixture/agent/approvals")
                        .await;
                if rows.as_array().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("cancellation must remove pending approval");
        let (status, _) =
            f.rt.post_json(
                &format!("/api/v1/apps/recovery_fixture/agent/approvals/{approval}"),
                &json!({"action":"approve"}),
            )
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{case}: stale confirmation");
        assert_eq!(f.records().await, 0, "{case}: cancelled action never ran");
        // Restart a stopped worker and reconstruct the same saved conversation.
        let events = result(f.invoke(session)).await;
        assert!(
            events.contains("event: done"),
            "{case}: valid recovery response: {events}"
        );
        assert_eq!(
            f.records().await,
            0,
            "{case}: pending action must not replay on resume"
        );
    }
    f.finish().await;
}

#[tokio::test]
async fn managed_restart_after_committed_action_keeps_result_without_duplicate_write() {
    for lost_result in [false, true] {
        let f = Fixture::boot("autonomous", !lost_result, lost_result).await;
        let session = Uuid::new_v4();
        let run = f.invoke(session);
        tokio::time::timeout(Duration::from_secs(20), f.after_write.notified())
            .await
            .expect("action must commit");
        assert_eq!(f.records().await, 1);
        f.rt.runtime
            .worker_manager()
            .stop_app("recovery_fixture")
            .await
            .unwrap();
        let _ = result(run).await;
        let events = result(f.invoke(session)).await;
        assert!(
            events.contains("event: done") && events.contains("Recovery context received"),
            "lost result={lost_result}: {events}"
        );
        assert_eq!(
            f.records().await,
            1,
            "lost result={lost_result}: committed write must not replay after worker restart"
        );
        let context = f.model_inputs.lock().unwrap().last().unwrap().to_string();
        if lost_result {
            assert!(
                context.contains("outcome is unknown"),
                "lost result must stay uncertain: {context}"
            );
        } else {
            assert!(
                context.contains("created_at"),
                "successful tool result survives restart: {context}"
            );
        }
        f.finish().await;
    }
}
