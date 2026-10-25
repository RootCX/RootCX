mod harness;

use reqwest::StatusCode;
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;

// Real HTTP, storage, agent worker, call_action and app worker; only the model is scripted.
#[tokio::test]
async fn agent_hands_an_attachment_to_an_app_action() {
    let rt = harness::TestRuntime::boot().await;
    rt.install_manifest(&json!({
        "appId":"business", "name":"Business", "version":"1.0.0",
        "actions":[{"id":"keep","name":"Keep a file","inputSchema":{"type":"object","properties":{
            "file":{"type":"string","format":"file"},"label":{"type":"string"}
        }}}],
        "dataContract":[]
    })).await;
    let backend = br#"serve({ rpc: {
      keep: async (input, _caller, ctx) => {
        const file = await ctx.downloadFile(input.file);
        return { fileId: file.fileId, name: file.name, text: new TextDecoder().decode(file.content), label: input.label };
      }
    } });"#;
    let (status, body) = rt.deploy("business", &harness::make_tar_gz(&[("index.ts", backend)])).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The scripted model hands over the first referenced file, then an unknown ID.
    let agent_backend = br#"
    import { createInterface } from "node:readline";
    const send = message => process.stdout.write(JSON.stringify(message) + "\n");
    const runs = new Map();
    function next(id) {
      const run = runs.get(id);
      if (run.tools.length) send({type:"agent_tool_call",invoke_id:id,call_id:crypto.randomUUID(),...run.tools.shift()});
      else { runs.delete(id); send({type:"agent_done",invoke_id:id,response:JSON.stringify(run),tokens:0}); }
    }
    createInterface({input:process.stdin}).on("line", line => {
      const m = JSON.parse(line);
      if (m.type === "discover") send({type:"discover",protocol:2});
      if (m.type === "agent_invoke") {
        const file = m.message.match(/file_id ([0-9a-f-]{36})/)?.[1];
        const keep = input => ({tool_name:"call_action",args:{app:"business",action:"keep",input}});
        if (!m.message.includes("file_id")) { send({type:"agent_done",invoke_id:m.invoke_id,response:JSON.stringify({history:m.history}),tokens:0}); return; }
        runs.set(m.invoke_id, {message:m.message, results:[], tools:[
          keep({file, label:"logo"}),
          keep({file:"00000000-0000-0000-0000-000000000000", label:"unknown"}),
          keep({file:m.message.match(/other ([0-9a-f-]{36})/)?.[1], label:"someone else's"}),
        ]});
        next(m.invoke_id);
      }
      if (m.type === "agent_tool_result") { runs.get(m.invoke_id).results.push({result:m.result,error:m.error}); next(m.invoke_id); }
    });
    "#;
    let config = br#"{"name":"Test assistant","supervision":{"mode":"autonomous"},"memory":{"enabled":true}}"#;
    let (status, body) = rt.deploy("assistant", &harness::make_tar_gz(&[("index.ts", agent_backend), ("agent.json", config)])).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let agent = rootcx_core::extensions::agents::agent_user_id("assistant");
    sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ('handoff_agent', ARRAY['*'])")
        .execute(rt.pool()).await.unwrap();
    sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, 'handoff_agent') ON CONFLICT DO NOTHING")
        .bind(agent).execute(rt.pool()).await.unwrap();

    let (status, upload) = rt.upload("/api/v1/apps/assistant/storage/upload", "devis.pdf", "application/pdf", b"%PDF-1.4 devis").await;
    assert_eq!(status, StatusCode::CREATED, "{upload}");
    let given = upload["file_id"].as_str().unwrap().to_string();

    // A file someone else gave the same agent.
    let other = Uuid::new_v4();
    sqlx::query("INSERT INTO rootcx_system.files (id, app_id, name, content_type, size, content, uploaded_by)
                 VALUES ($1, 'assistant', 'autre.pdf', 'application/pdf', 3, 'abc', gen_random_uuid())")
        .bind(other).execute(rt.pool()).await.unwrap();

    let response = rt.client.post(rt.url("/api/v1/apps/assistant/agent/invoke"))
        .bearer_auth(&rt.token)
        .json(&json!({"message":format!("Voici mon ancien devis (other {other})"),"file_ids":[given]}))
        .timeout(Duration::from_secs(30)).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let events = response.text().await.unwrap();
    let done = events.split("\n\n").find(|e| e.lines().any(|l| l.trim() == "event: done"))
        .unwrap_or_else(|| panic!("missing result: {events}"));
    let data: Value = serde_json::from_str(done.lines().find_map(|l| l.strip_prefix("data:")).unwrap()).unwrap();
    let run: Value = serde_json::from_str(data["response"].as_str().unwrap()).unwrap();

    let message = run["message"].as_str().unwrap();
    assert!(message.starts_with(&format!("Voici mon ancien devis (other {other})\n\n")), "{message}");
    assert!(message.contains(&format!("- devis.pdf (application/pdf): file_id {given}")), "{message}");

    // History keeps the references, so a later turn can still hand the file over.
    let remembered: String = sqlx::query_scalar("SELECT content FROM rootcx_system.agent_messages WHERE role = 'user'")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(remembered, message);

    // A turn stored empty by an older Core is left out of the next turn's history.
    let session = data["session_id"].as_str().unwrap();
    sqlx::query("INSERT INTO rootcx_system.agent_messages (id, session_id, role, content) VALUES (gen_random_uuid(), $1::uuid, 'user', '')")
        .bind(session).execute(rt.pool()).await.unwrap();
    let response = rt.client.post(rt.url("/api/v1/apps/assistant/agent/invoke"))
        .bearer_auth(&rt.token).json(&json!({"message":"Et maintenant ?","session_id":session}))
        .timeout(Duration::from_secs(30)).send().await.unwrap();
    let events = response.text().await.unwrap();
    let done = events.split("\n\n").find(|e| e.lines().any(|l| l.trim() == "event: done")).unwrap_or_else(|| panic!("{events}"));
    let next: Value = serde_json::from_str(done.lines().find_map(|l| l.strip_prefix("data:")).unwrap()).unwrap();
    let history: Value = serde_json::from_str::<Value>(next["response"].as_str().unwrap()).unwrap()["history"].clone();
    let contents: Vec<&str> = history.as_array().unwrap().iter().map(|m| m["content"].as_str().unwrap()).collect();
    assert_eq!(contents.len(), 2, "{history}");
    assert!(contents.iter().all(|c| !c.is_empty()), "{history}");

    let kept = &run["results"][0];
    assert!(kept["error"].is_null(), "{kept}");
    let kept = &kept["result"];
    assert_eq!((&kept["name"], &kept["text"], &kept["label"]), (&json!("devis.pdf"), &json!("%PDF-1.4 devis"), &json!("logo")));
    let copy: Uuid = kept["fileId"].as_str().unwrap().parse().unwrap();
    assert_ne!(copy.to_string(), given, "the app receives its own copy");
    let owners: Vec<(String,)> = sqlx::query_as("SELECT app_id FROM rootcx_system.files WHERE id = ANY($1) ORDER BY app_id")
        .bind(vec![copy, given.parse::<Uuid>().unwrap()]).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(owners, vec![("assistant".into(),), ("business".into(),)], "the original stays with the agent");

    // Not a file this person gave the agent: passed through, so the app reads only its own storage.
    for refused in [&run["results"][1], &run["results"][2]] {
        assert!(refused["result"].is_null(), "{refused}");
        assert!(refused["error"].as_str().unwrap_or_default().contains("not found"), "{refused}");
    }
    let copies: i64 = sqlx::query_scalar("SELECT count(*) FROM rootcx_system.files WHERE app_id = 'business'")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(copies, 1);
}
