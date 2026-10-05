mod harness;

use std::{sync::{Arc, atomic::{AtomicBool, Ordering}}, time::Duration};
use axum::{Json, Router, routing::post};
use hmac::{Hmac, Mac};
use reqwest::{RequestBuilder, StatusCode};
use serde_json::{Value, json};
use sha2::Sha256;
use tokio::sync::mpsc::Receiver;
use uuid::Uuid;

fn webhook(rt: &harness::TestRuntime, provider: &str, channel: Uuid, body: Value) -> RequestBuilder {
    let body = body.to_string();
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"isolated-test-key").unwrap();
    mac.update(format!("core:{channel}\n{timestamp}\n{body}").as_bytes());
    rt.client.post(rt.url(&format!("/api/v1/channels/{provider}/{channel}/webhook")))
        .header("content-type", "application/json")
        .header("x-rootcx-timestamp", timestamp)
        .header("x-rootcx-signature", hex::encode(mac.finalize().into_bytes()))
        .body(body).timeout(Duration::from_secs(30))
}

fn invoke(rt: &harness::TestRuntime, provider: &str, channel: Uuid, grant: &str, tools: Value) -> tokio::task::JoinHandle<reqwest::Response> {
    let request = webhook(rt, provider, channel, json!({"id":Uuid::new_v4(),"chat_id":grant,"text":tools.to_string()}));
    tokio::spawn(async move { request.send().await.unwrap() })
}

async fn next_message(rx: &mut Receiver<Value>) -> Value {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let message = rx.recv().await.expect("gateway closed");
            // Acknowledgements are independent of the agent's eventual result.
            let text = message.pointer("/text/body").and_then(Value::as_str).unwrap_or("");
            if message["type"] == "interactive" || text.starts_with('[') { return message; }
        }
    }).await.expect("channel message timed out")
}

fn results(message: &Value) -> Vec<Value> {
    serde_json::from_str(message.pointer("/text/body").and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected final agent result: {message}"))).unwrap()
}

async fn web_results(rt: &harness::TestRuntime, token: &str, tools: &Value) -> (StatusCode, Vec<Value>) {
    let response = rt.client.post(rt.url("/api/v1/apps/assistant/agent/invoke"))
        .bearer_auth(token).json(&json!({"message":tools.to_string()}))
        .timeout(Duration::from_secs(20)).send().await.unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    (status, parse_web_results(&body))
}

fn parse_web_results(body: &str) -> Vec<Value> {
    let done = body.split("\n\n").find(|event| event.lines().any(|line| line.trim() == "event: done"))
        .unwrap_or_else(|| panic!("missing Web result: {body}"));
    let data: Value = serde_json::from_str(done.lines().find_map(|line| line.strip_prefix("data:")).unwrap()).unwrap();
    serde_json::from_str(data["response"].as_str().unwrap()).unwrap()
}

async fn permissions(rt: &harness::TestRuntime, role: &str, perms: &[&str]) {
    sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = $2 WHERE name = $1")
        .bind(role).bind(perms).execute(rt.pool()).await.unwrap();
}

async fn count(rt: &harness::TestRuntime) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM business.records").fetch_one(rt.pool()).await.unwrap()
}

// A separate binary isolates the channel's process environment. Core's HTTP,
// signed confirmations, worker RPC and PostgreSQL run; the external gateway is simulated.
#[tokio::test]
async fn web_and_managed_channels_preserve_delegated_authority() {
    for provider in ["whatsapp", "telegram"] {
        eprintln!("managed authority provider={provider}");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let gateway_url = format!("http://{}", listener.local_addr().unwrap());
        unsafe {
            std::env::set_var("ROOTCX_TENANT_REF", "whatsapp-test");
            std::env::set_var("ROOTCX_WHATSAPP_TENANT_REF", "whatsapp-test");
            std::env::set_var("ROOTCX_WHATSAPP_GATEWAY_URL", &gateway_url);
            std::env::set_var("ROOTCX_WHATSAPP_RELAY_KEY", "isolated-test-key");
            std::env::set_var("ROOTCX_WHATSAPP_PHONE_NUMBER", "15550000000");
            std::env::set_var("ROOTCX_CHANNEL_TELEGRAM_USERNAME", "isolated_shappy_bot");
        }
        let authorized = Arc::new(AtomicBool::new(true));
        let access = authorized.clone();
        let (sent, mut messages) = tokio::sync::mpsc::channel(32);
        let gateway = tokio::spawn(async move {
            let router = Router::new()
                .route("/authorize", post(move |Json(body): Json<Value>| {
                    let access = access.clone();
                    async move { Json(json!({"authorized":access.load(Ordering::SeqCst),"grantId":body["to"],"coreUserId":body["coreUserId"]})) }
                }))
                .route("/send", post(move |Json(mut body): Json<Value>| {
                    let sent = sent.clone();
                    async move {
                        assert_eq!(body["provider"], provider);
                        if body["kind"] == "text" { body["type"] = json!("text"); body["text"] = json!({"body":body["text"].clone()}); }
                        if body["kind"] == "confirmation" {
                            body["type"] = json!("interactive");
                            body["interactive"] = json!({"body":{"text":body["text"]},"action":{"buttons":body["choices"].as_array().unwrap().iter().map(|b|json!({"reply":b})).collect::<Vec<_>>()}});
                        }
                        sent.send(body).await.unwrap(); Json(json!({"ok":true}))
                    }
                }));
            axum::serve(listener, router).await.unwrap();
        });
        let rt = harness::TestRuntime::boot().await;
        rt.install_manifest(&json!({
            "appId":"business", "name":"Business", "version":"1.0.0",
            "actions":[{"id":"create_record","name":"Create record","inputSchema":{"type":"object"}}],
            "dataContract":[{"entityName":"records","fields":[{"name":"name","type":"text"}]}]
        })).await;
        let backend = br#"serve({ rpc: {
          create_record: async (input, _caller, ctx) => ctx.collection("records").create(input)
        } });"#;
        let (status, body) = rt.deploy("business", &harness::make_tar_gz(&[("index.ts", backend)])).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let agent_backend = br#"
    import { createInterface } from "node:readline";
    const send = message => process.stdout.write(JSON.stringify(message) + "\n");
    const runs = new Map();
    function next(id) {
      const run = runs.get(id);
      if (run.tools.length) send({type:"agent_tool_call",invoke_id:id,call_id:crypto.randomUUID(),...run.tools.shift()});
      else { runs.delete(id); send({type:"agent_done",invoke_id:id,response:JSON.stringify(run.results),tokens:0}); }
    }
    createInterface({input:process.stdin}).on("line", line => {
      const m = JSON.parse(line);
      if (m.type === "discover") send({type:"discover",protocol:2});
      if (m.type === "agent_invoke") {
        const tools = JSON.parse(m.message);
        if (tools === null) send({type:"agent_done",invoke_id:m.invoke_id,response:JSON.stringify([{pid:process.pid}]),tokens:0});
        else { runs.set(m.invoke_id,{tools,results:[]}); next(m.invoke_id); }
      }
      if (m.type === "agent_tool_result") { runs.get(m.invoke_id).results.push({result:m.result,error:m.error}); next(m.invoke_id); }
    });
    "#;
        let config = br#"{"name":"Test assistant","supervision":{"mode":"autonomous","policies":[{"action":"call_action","rateLimit":{"max":1,"window":"1h"}}]}}"#;
        let (status, body) = rt.deploy("assistant", &harness::make_tar_gz(&[("index.ts", agent_backend), ("agent.json", config)])).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let token = rt.create_user("operator@test.local").await;
        let user: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'operator@test.local'")
            .fetch_one(rt.pool()).await.unwrap();
        let agent = rootcx_core::extensions::agents::agent_user_id("assistant");
        for (uid, role) in [(user, "whatsapp_human"), (agent, "whatsapp_agent")] {
            sqlx::query("DELETE FROM rootcx_system.rbac_assignments WHERE user_id = $1").bind(uid).execute(rt.pool()).await.unwrap();
            sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, ARRAY['*'])")
                .bind(role).execute(rt.pool()).await.unwrap();
            sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
                .bind(uid).bind(role).execute(rt.pool()).await.unwrap();
        }
        let channel: Uuid = sqlx::query_scalar("SELECT id FROM rootcx_system.channels WHERE provider = $1")
            .bind(provider).fetch_one(rt.pool()).await.unwrap();
        let grant = Uuid::new_v4().to_string();
        let link_route = if provider == "whatsapp" { "whatsapp-link" } else { "managed-link" };
        let (status, body) = rt.request_as(reqwest::Method::POST,
            &format!("/api/v1/channels/{channel}/{link_route}"), &token, Some(&json!({"grantId":grant}))).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let discovery = invoke(&rt, provider, channel, &grant, json!([{"tool_name":"list_actions","args":{}}]));
        let output = results(&next_message(&mut messages).await);
        assert!(output[0]["error"].is_null(), "{output:?}");
        assert!(output[0]["result"].as_array().unwrap().iter().any(|app| app["appId"] == "business"),
            "discovery must not need collection grants");
        assert_eq!(discovery.await.unwrap().status(), StatusCode::OK);

        // Untrusted worker code sees invocation IDs. Distinct origins must have
        // different processes even for the same human and identical permissions.
        let (status, web_process) = web_results(&rt, &token, &Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{provider}: Web invocation");
        let request = invoke(&rt, provider, channel, &grant, Value::Null);
        let channel_process = results(&next_message(&mut messages).await);
        assert_eq!(request.await.unwrap().status(), StatusCode::OK);
        assert_ne!(web_process[0]["pid"], channel_process[0]["pid"], "origins cannot borrow invocation IDs from a shared worker");
        let (status, repeated_web_process) = web_results(&rt, &token, &Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{provider}: repeated Web invocation");
        assert_eq!(repeated_web_process, web_process, "{provider}: same origin still reuses its worker");

        // Autonomous mode has the same behavior on Web and WhatsApp, including a
        // formerly blocked tool and ordinary tool validation (no external requests).
        for tools in [
            json!([{"tool_name":"list_integrations","args":{}}]),
            json!([{"tool_name":"http_request","args":{"url":"http://127.0.0.1:1"}}]),
            json!([{"tool_name":"call_integration","args":{}}]),
        ] {
            let (status, web) = web_results(&rt, &token, &tools).await;
            assert_eq!(status, StatusCode::OK, "{provider}: {tools}");
            let request = invoke(&rt, provider, channel, &grant, tools);
            let whatsapp = results(&next_message(&mut messages).await);
            assert_eq!(whatsapp, web, "transport must not change tool authority or validation");
            assert_eq!(request.await.unwrap().status(), StatusCode::OK);
        }
        let autonomous_action = json!([{"tool_name":"call_action","args":{"app":"business","action":"create_record","input":{"name":"autonomous"}}}]);
        let before = count(&rt).await;
        let (status, autonomous_web) = web_results(&rt, &token, &autonomous_action).await;
        assert_eq!(status, StatusCode::OK, "{provider}: autonomous Web action");
        assert!(autonomous_web[0]["error"].is_null(), "{provider}: autonomous Web action");
        let request = invoke(&rt, provider, channel, &grant, autonomous_action);
        assert!(results(&next_message(&mut messages).await)[0]["error"].is_null(), "WhatsApp must not add an approval to autonomous mode");
        assert_eq!(request.await.unwrap().status(), StatusCode::OK);
        assert_eq!(count(&rt).await, before + 2);

        let supervised = br#"{"name":"Supervised agent","supervision":{"mode":"supervised","policies":[{"action":"call_action","requires":"approval","rateLimit":{"max":1,"window":"1h"}}]}}"#;
        let (status, body) = rt.deploy("assistant", &harness::make_tar_gz(&[("index.ts", agent_backend), ("agent.json", supervised)])).await;
        assert_eq!(status, StatusCode::OK, "{body}");

        let base = ["app:assistant:invoke", "tool:list_actions", "tool:call_action", "app:business:action:create_record", "app:business:records.create", "app:business:records.read"];
        for case in ["admin", "member", "rejected", "no action right", "no data right", "no tool right", "human revoked", "agent revoked", "channel revoked", "gateway revoked", "new right"] {
            authorized.store(true, Ordering::SeqCst);
            permissions(&rt, "whatsapp_agent", &["app:*", "tool:*"]).await;
            let initial: Vec<&str> = match case {
                "admin" => vec!["*"],
                "no action right" | "new right" => base.iter().copied().filter(|p| *p != "app:business:action:create_record").collect(),
                "no data right" => base.iter().copied().filter(|p| *p != "app:business:records.create").collect(),
                "no tool right" => base.iter().copied().filter(|p| *p != "tool:call_action").collect(),
                _ => base.to_vec(),
            };
            permissions(&rt, "whatsapp_human", &initial).await;
            sqlx::query("UPDATE rootcx_system.channels SET status = 'active' WHERE id = $1").bind(channel).execute(rt.pool()).await.unwrap();
            let before = count(&rt).await;
            let tool = json!({"tool_name":"call_action","args":{"app":"business","action":"create_record","input":{"name":case}}});
            let steps = if case == "member" { json!([tool.clone(), tool.clone()]) } else { json!([tool.clone()]) };
            let request = invoke(&rt, provider, channel, &grant, steps);
            let approval = next_message(&mut messages).await;
            assert_eq!(approval["type"], "interactive", "{provider}/{case}: action must wait for confirmation: {approval}");
            let description = approval.pointer("/interactive/body/text").unwrap().as_str().unwrap();
            let shown_args: Value = serde_json::from_str(description.split_once('\n').unwrap().1.rsplit_once("\n\n").unwrap().0).unwrap();
            assert_eq!(shown_args, tool["args"], "{provider}/{case}: exact action arguments must be visible");
            assert_eq!(count(&rt).await, before, "{provider}/{case}: no write before confirmation");
            let button = if case == "rejected" { 1 } else { 0 };
            let callback = approval["interactive"]["action"]["buttons"][button]["reply"]["id"].as_str().unwrap();
            let approval_id = callback.split(':').nth(1).unwrap();
            let wrong = webhook(&rt, provider, channel, json!({"id":Uuid::new_v4(),"chat_id":Uuid::new_v4(),"callback":callback})).send().await.unwrap();
            assert_eq!(wrong.status(), StatusCode::BAD_REQUEST, "{provider}/{case}: signed button belongs to its original identity");
            match case {
                "human revoked" => permissions(&rt, "whatsapp_human", &base[..3]).await,
                "agent revoked" => permissions(&rt, "whatsapp_agent", &["tool:*"]).await,
                "new right" => permissions(&rt, "whatsapp_human", &base).await,
                "channel revoked" => { sqlx::query("UPDATE rootcx_system.channels SET status = 'disabled' WHERE id = $1").bind(channel).execute(rt.pool()).await.unwrap(); }
                "gateway revoked" => authorized.store(false, Ordering::SeqCst),
                _ => {}
            }
            let reply = webhook(&rt, provider, channel, json!({"id":Uuid::new_v4(),"chat_id":grant,"callback":callback})).send().await.unwrap();
            assert_eq!(reply.status(), if case == "gateway revoked" { StatusCode::FORBIDDEN } else { StatusCode::OK }, "{provider}/{case}");
            if matches!(case, "channel revoked" | "gateway revoked") {
                // The callback itself was refused. End the pending fixture without
                // waiting for its ten-minute expiry; cancellation never executes it.
                rt.runtime.pending_approvals().cancel(approval_id, "test cleanup after revocation").await;
            }
            let output = results(&next_message(&mut messages).await);
            let allowed = matches!(case, "admin" | "member");
            assert_eq!(output[0]["error"].is_null(), allowed, "{provider}/{case}: {output:?}");
            if case == "member" {
                assert_eq!(output.len(), 2);
                assert!(output[1]["error"].as_str().unwrap().contains("rate limited"), "confirmation must not bypass rate limits: {output:?}");
            }
            assert_eq!(request.await.unwrap().status(), StatusCode::OK, "{provider}/{case}");
            assert_eq!(count(&rt).await, before + i64::from(allowed), "{provider}/{case}: database outcome");
        }
        authorized.store(true, Ordering::SeqCst);
        permissions(&rt, "whatsapp_human", &["*"]).await;
        rt.install_manifest(&json!({"appId":"helper","name":"Helper","version":"1.0.0","type":"agent","dataContract":[]})).await;
        let (status, body) = rt.deploy("helper", &harness::make_tar_gz(&[("index.ts", agent_backend), ("agent.json", supervised)])).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        for case in ["child allowed", "parent revoked", "child revoked", "human revoked"] {
            permissions(&rt, "whatsapp_agent", &["app:*", "tool:*"]).await;
            permissions(&rt, "whatsapp_human", &["*"]).await;
            permissions(&rt, "app:helper:agent", &["app:*", "tool:*"]).await;
            let before = count(&rt).await;
            let child_tools = json!([{"tool_name":"call_action","args":{"app":"business","action":"create_record","input":{"name":case}}}]);
            let request = invoke(&rt, provider, channel, &grant, json!([{"tool_name":"invoke_agent","args":{"app_id":"helper","message":child_tools.to_string()}}]));
            let approval = next_message(&mut messages).await;
            assert_eq!(approval["type"], "interactive", "{provider}/{case}: child supervision must reach WhatsApp");
            assert_eq!(count(&rt).await, before, "{provider}/{case}: child must await confirmation");
            assert!(rt.runtime.pending_approvals().list("assistant", &rootcx_core::governance::execution::ApprovalOwner { user_id: user, origin: None }).await.is_empty(), "child's WhatsApp approval must stay isolated from generic routes");
            match case {
                "parent revoked" => permissions(&rt, "whatsapp_agent", &["tool:*", "app:helper:invoke"]).await,
                "child revoked" => permissions(&rt, "app:helper:agent", &["tool:*"]).await,
                "human revoked" => permissions(&rt, "whatsapp_human", &["tool:*", "app:assistant:invoke", "app:helper:invoke"]).await,
                _ => {}
            }
            let callback = &approval["interactive"]["action"]["buttons"][0]["reply"]["id"];
            let reply = webhook(&rt, provider, channel, json!({"id":Uuid::new_v4(),"chat_id":grant,"callback":callback})).send().await.unwrap();
            assert_eq!(reply.status(), StatusCode::OK, "{provider}/{case}");
            let output = results(&next_message(&mut messages).await);
            let child: Vec<Value> = serde_json::from_str(output[0]["result"]["response"].as_str().unwrap()).unwrap();
            assert_eq!(child[0]["error"].is_null(), case == "child allowed", "{provider}/{case}: {child:?}");
            assert_eq!(request.await.unwrap().status(), StatusCode::OK);
            assert_eq!(count(&rt).await, before + i64::from(case == "child allowed"), "{provider}/{case}");
        }
        // Exercise the same live authority and confirmation ownership through Web,
        // including a child's confirmation presented on the parent's conversation.
        let stranger = rt.create_user("other-reviewer@test.local").await;
        for (nested, case) in [(false, "allowed"), (false, "human revoked"), (false, "agent revoked"),
            (false, "new right"), (false, "disconnected"), (true, "allowed"), (true, "parent revoked"), (true, "child revoked")] {
            let mut rights = base.to_vec();
            if case == "new right" { rights.retain(|p| *p != "app:business:action:create_record"); }
            rights.push("tool:invoke_agent"); rights.push("app:helper:invoke");
            permissions(&rt, "whatsapp_human", &rights).await;
            permissions(&rt, "whatsapp_agent", &["app:*", "tool:*"]).await;
            permissions(&rt, "app:helper:agent", &["app:*", "tool:*"]).await;
            let action = json!([{"tool_name":"call_action","args":{"app":"business","action":"create_record","input":{"name":case}}}]);
            let steps = if nested { json!([{"tool_name":"invoke_agent","args":{"app_id":"helper","message":action.to_string()}}]) } else { action };
            let before = count(&rt).await;
            let response = rt.client.post(rt.url("/api/v1/apps/assistant/agent/invoke"))
                .bearer_auth(&token).json(&json!({"message":steps.to_string()}))
                .timeout(Duration::from_secs(20)).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{provider}/Web {nested} {case}");
            let result = tokio::spawn(async move { response.text().await.unwrap() });
            let path = "/api/v1/apps/assistant/agent/approvals";
            let approval = tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    let (status, pending) = rt.request_as(reqwest::Method::GET, path, &token, None).await;
                    assert_eq!(status, StatusCode::OK);
                    if let Some(approval) = pending.as_array().unwrap().first() { break approval.clone(); }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.expect("Web approval must be visible to the initiating human");
            assert_eq!(count(&rt).await, before, "{provider}/Web {nested} {case}: no write before consent");
            if case == "disconnected" {
                result.abort();
                let _ = result.await;
                tokio::time::timeout(Duration::from_secs(10), async {
                    loop {
                        let (_, pending) = rt.request_as(reqwest::Method::GET, path, &token, None).await;
                        if pending == json!([]) { break; }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                }).await.expect("disconnect must cancel the pending confirmation");
                assert_eq!(count(&rt).await, before, "disconnected request cannot execute");
                continue;
            }
            let approval_path = format!("{path}/{}", approval["approvalId"].as_str().unwrap());
            let (status, visible) = rt.request_as(reqwest::Method::GET, path, &stranger, None).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(visible, json!([]), "another human cannot discover pending approvals");
            let (status, _) = rt.request_as(reqwest::Method::POST, &approval_path, &stranger, Some(&json!({"action":"approve"}))).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "another human cannot consume the approval");
            match case {
                "human revoked" => permissions(&rt, "whatsapp_human", &base[..3]).await,
                "agent revoked" | "parent revoked" => permissions(&rt, "whatsapp_agent", &["tool:*", "app:helper:invoke"]).await,
                "child revoked" => permissions(&rt, "app:helper:agent", &["tool:*"]).await,
                "new right" => permissions(&rt, "whatsapp_human", &base).await,
                _ => {}
            }
            let (status, _) = rt.request_as(reqwest::Method::POST, &approval_path, &token, Some(&json!({"action":"approve"}))).await;
            assert_eq!(status, StatusCode::OK, "{provider}/Web {nested} {case}");
            let output = parse_web_results(&result.await.unwrap());
            let tool_results = if nested { serde_json::from_str::<Vec<Value>>(output[0]["result"]["response"].as_str().unwrap()).unwrap() } else { output };
            assert_eq!(tool_results[0]["error"].is_null(), case == "allowed", "{provider}/Web {nested} {case}: {tool_results:?}");
            assert_eq!(count(&rt).await, before + i64::from(case == "allowed"), "{provider}/Web {nested} {case}: database outcome");
            let (status, _) = rt.request_as(reqwest::Method::POST, &approval_path, &token, Some(&json!({"action":"approve"}))).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "confirmation cannot be replayed");
        }
        rt.shutdown().await;
        gateway.abort();
    }
}
