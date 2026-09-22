use crate::harness::TestRuntime;
use reqwest::{Method, StatusCode};
use rootcx_core::tools::{Tool, ToolContext};
use serde_json::{Value, json};

const AGENT_TOOLS: [&str; 3] = ["call_action", "invoke_agent", "call_integration"];

fn tool_graph(tool: &str, params: Value) -> Value {
    json!({
        "nodes": [
            {"id": "start", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0, 0]},
            {"id": "operation", "kind": {"type": "tool", "toolName": tool}, "params": params, "position": [1, 0]}
        ],
        "edges": [{"from": "start", "to": "operation", "fromOutput": 0}]
    })
}

#[tokio::test]
async fn discovery_matches_executor_availability_without_narrowing_agents() {
    let rt = TestRuntime::boot().await;
    let (status, http) = rt.get_json("/api/v1/tools").await;
    assert_eq!(status, StatusCode::OK, "{http}");
    let (status, palette) = rt.get_json("/api/v1/workflows/nodes").await;
    assert_eq!(status, StatusCode::OK, "{palette}");
    let agent = rt.runtime.tool_registry().descriptors_for_permissions(&["*".into()], &json!([]));

    for tool in AGENT_TOOLS {
        for (surface, tools) in [("HTTP", &http), ("workflow", &palette["tools"])] {
            assert!(
                tools.as_array().unwrap().iter().all(|entry| entry["name"] != tool),
                "{surface} advertises unavailable {tool}: {tools}",
            );
        }
        assert!(agent.iter().any(|entry| entry.name == tool), "agent lost {tool}");
    }
    for tool in ["query_data", "mutate_data"] {
        for (surface, tools) in [("HTTP", &http), ("workflow", &palette["tools"])] {
            assert!(
                tools.as_array().unwrap().iter().any(|entry| entry["name"] == tool),
                "{surface} lost supported {tool}: {tools}",
            );
        }
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn http_rejects_agent_tools_after_auth_checks_but_executes_data_tools() {
    let rt = TestRuntime::boot().await;
    rt.install("tooldata", "contacts").await;
    let record = rt.create("tooldata", "contacts", &json!({"first_name": "Ada", "last_name": "L"})).await;
    let restricted = rt.create_user("restricted-tools@test.local").await;
    sqlx::query(
        "DELETE FROM rootcx_system.rbac_assignments WHERE user_id = (
            SELECT id FROM rootcx_system.users WHERE email = 'restricted-tools@test.local'
        )",
    ).execute(rt.pool()).await.unwrap();

    for tool in AGENT_TOOLS {
        let path = format!("/api/v1/tools/{tool}/execute");
        let request = json!({"appId": "tooldata", "args": {}});
        let (status, body) = rt.post_unauthed(&path, &request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "anonymous {tool}: {body}");
        let (status, body) = rt.request_as(Method::POST, &path, &restricted, Some(&request)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "unprivileged {tool}: {body}");
        let (status, body) = rt.post_json(&path, &request).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "authorized {tool}: {body}");
        let error = body["error"].as_str().unwrap();
        assert!(
            error.contains(tool) && error.contains("requires agent execution context"),
            "{tool} must fail with actionable availability guidance: {body}",
        );
    }

    let (status, rows) = rt.post_json("/api/v1/tools/query_data/execute", &json!({
        "appId": "tooldata", "args": {"entity": "contacts"}
    })).await;
    assert_eq!(status, StatusCode::OK, "same-app query_data remains executable: {rows}");
    assert_eq!(rows.as_array().unwrap().len(), 1, "{rows}");
    assert_eq!(rows[0]["id"], record["id"], "{rows}");

    let (status, created) = rt.post_json("/api/v1/tools/mutate_data/execute", &json!({
        "appId": "tooldata", "args": {
            "entity": "contacts", "action": "create",
            "data": {"first_name": "Grace", "last_name": "H"}
        }
    })).await;
    assert_eq!(status, StatusCode::OK, "same-app mutate_data remains executable: {created}");
    let persisted: (String, String) = sqlx::query_as(
        "SELECT first_name, last_name FROM tooldata.contacts WHERE id = $1::uuid",
    ).bind(created["id"].as_str().expect("create returns a record id"))
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(persisted, ("Grace".into(), "H".into()), "HTTP mutation must persist the returned row");

    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, inherits, permissions)
         VALUES ('tool_only_data', '{}', ARRAY['tool:query_data', 'tool:mutate_data'])",
    ).execute(rt.pool()).await.unwrap();
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
         SELECT id, 'tool_only_data' FROM rootcx_system.users WHERE email = 'restricted-tools@test.local'",
    ).execute(rt.pool()).await.unwrap();
    let (status, rows) = rt.request_as(
        Method::POST, "/api/v1/tools/query_data/execute", &restricted,
        Some(&json!({"appId": "tooldata", "args": {"entity": "contacts"}})),
    ).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(rows, json!([]), "tool permission alone must not expose provider rows");
    let (status, body) = rt.request_as(
        Method::POST, "/api/v1/tools/mutate_data/execute", &restricted,
        Some(&json!({"appId": "tooldata", "args": {
            "entity": "contacts", "action": "create",
            "data": {"first_name": "Forbidden", "last_name": "Write"}
        }})),
    ).await;
    // The HTTP adapter currently maps tool execution errors, including RLS denial, to 500.
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "tool permission cannot authorize an entity write: {body}");
    let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tooldata.contacts")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(row_count, 2, "denied mutation must leave only the two authorized records");
    let forbidden_count: i64 = sqlx::query_scalar("SELECT count(*) FROM tooldata.contacts WHERE first_name = 'Forbidden'")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(forbidden_count, 0, "tool-only caller must not persist a row");

    // HTTP's authenticated-user fallback must never give a bound agent authority
    // when its responsible human is missing, even if the actor is an admin.
    let actor = sqlx::query_scalar("SELECT id FROM rootcx_system.users WHERE email = 'admin@test.local'")
        .fetch_one(rt.pool()).await.unwrap();
    let mut bound = ToolContext {
        pool: rt.pool().clone(),
        core_bound_app_id: Some("tooldata".into()),
        app_id: "tooldata".into(),
        user_id: actor,
        invoker_user_id: None,
        permissions: vec!["*".into()],
        task_scope: None,
        args: json!({"entity": "contacts"}),
        agent_dispatch: None,
        integration_caller: None,
        action_caller: None,
        stream_tx: None,
        idempotency_key: None,
    };
    let rows = rootcx_core::tools::query_data::QueryDataTool.execute(&bound).await.unwrap();
    assert_eq!(rows, json!([]), "bound actor without a human must not read existing rows");
    bound.args = json!({
        "entity": "contacts", "action": "create",
        "data": {"first_name": "BoundForbidden", "last_name": "Write"}
    });
    let result = rootcx_core::tools::mutate_data::MutateDataTool.execute(&bound).await;
    assert!(result.is_err(), "bound actor without a human must not write: {result:?}");
    let (row_count, forbidden_count): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE first_name = 'BoundForbidden') FROM tooldata.contacts",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!((row_count, forbidden_count), (2, 0), "bound denial must persist no write");

    // An active provider grant must not turn the actor into a responsible human.
    rt.install("toolprovider", "contacts").await;
    let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &json!({
        "consumerApp": "tooldata", "providerApp": "toolprovider", "entity": "contacts",
        "actions": ["create"], "fields": ["first_name", "last_name"],
        "writeFields": ["first_name", "last_name"],
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let (status, approved) = rt.post_json(
        &format!("/api/v1/cross-app/grants/{}/approve", grant["id"].as_str().unwrap()),
        &json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    assert_eq!(approved["status"], "active", "{approved}");
    bound.args = json!({
        "app": "toolprovider", "entity": "contacts", "action": "create",
        "data": {"first_name": "RemoteForbidden", "last_name": "Write"}
    });
    let result = rootcx_core::tools::mutate_data::MutateDataTool.execute(&bound).await;
    assert!(result.is_err(), "active write grant must not supply a missing human: {result:?}");
    let row_count: i64 = sqlx::query_scalar("SELECT count(*) FROM toolprovider.contacts")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(row_count, 0, "missing-human remote mutation must persist no rows");

    // The same grant and permission ceiling remain usable with a responsible human.
    bound.invoker_user_id = Some(actor);
    bound.args["data"]["first_name"] = json!("RemoteAllowed");
    let created = rootcx_core::tools::mutate_data::MutateDataTool.execute(&bound).await
        .expect("bound remote mutation with a human must succeed");
    let persisted: (String, String) = sqlx::query_as(
        "SELECT first_name, last_name FROM toolprovider.contacts WHERE id = $1::uuid",
    ).bind(created["id"].as_str().expect("remote create returns a record id"))
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(persisted, ("RemoteAllowed".into(), "Write".into()));
    let (row_count, forbidden_count): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE first_name = 'RemoteForbidden') FROM toolprovider.contacts",
    ).fetch_one(rt.pool()).await.unwrap();
    assert_eq!((row_count, forbidden_count), (1, 0), "only the authorized remote write may persist");

    let (status, body) = rt.post_json("/api/v1/tools/query_data/execute", &json!({
        "appId": "claimed-consumer", "args": {"app": "tooldata", "entity": "contacts"}
    })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "HTTP cannot claim collection grant authority: {body}");
    rt.shutdown().await;
}

#[tokio::test]
async fn workflow_saves_refuse_unavailable_tools_and_granted_data_nodes_still_run() {
    let rt = TestRuntime::boot().await;
    rt.install("tooldata", "contacts").await;
    let record = rt.create("tooldata", "contacts", &json!({"first_name": "Ada", "last_name": "L"})).await;
    let graph = tool_graph("query_data", json!({"app": "tooldata", "entity": "contacts"}));
    let (status, created) = rt.post_json("/api/v1/workflows", &json!({
        "name": "available-data", "graph": graph,
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap();
    let path = format!("/api/v1/workflows/{id}");
    let (_, before) = rt.get_json(&path).await;
    let app_count: i64 = sqlx::query_scalar("SELECT count(*) FROM rootcx_system.apps")
        .fetch_one(rt.pool()).await.unwrap();

    for tool in AGENT_TOOLS.into_iter().chain(["unknown_tool", ""]) {
        let invalid = tool_graph(tool, json!({}));
        let (status, body) = rt.post_json("/api/v1/workflows", &json!({
            "name": format!("unavailable-{tool}"), "graph": invalid,
        })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "create {tool}: {body}");
        let expected = if AGENT_TOOLS.contains(&tool) { "requires agent execution context" } else { "unknown tool" };
        let error = body["error"].as_str().unwrap();
        assert!(error.contains("operation") && error.contains(tool) && error.contains(expected), "{tool}: {body}");

        let (status, body) = rt.put_json(&path, &json!({
            "name": "must-not-change", "graph": invalid, "enabled": true,
        })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "update {tool}: {body}");
        assert!(body["error"].as_str().unwrap().contains(expected), "{tool}: {body}");
        let (_, after) = rt.get_json(&path).await;
        assert_eq!(after, before, "rejected {tool} save must not change graph, name, enabled state or version");
    }
    let after_count: i64 = sqlx::query_scalar("SELECT count(*) FROM rootcx_system.apps")
        .fetch_one(rt.pool()).await.unwrap();
    assert_eq!(after_count, app_count, "rejected creates must not provision backing apps");

    // Legacy saved graphs must also be checked before enabling.
    sqlx::query("UPDATE rootcx_system.workflows SET graph = $2 WHERE id = $1::uuid")
        .bind(id).bind(tool_graph("call_action", json!({}))).execute(rt.pool()).await.unwrap();
    let (status, body) = rt.put_json(&path, &json!({"enabled": true})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "legacy enable: {body}");
    assert!(body["error"].as_str().unwrap().contains("requires agent execution context"), "{body}");

    let consumer: String = sqlx::query_scalar("SELECT app_id FROM rootcx_system.workflows WHERE id = $1::uuid")
        .bind(id).fetch_one(rt.pool()).await.unwrap();
    let (status, grant) = rt.post_json("/api/v1/cross-app/grants", &json!({
        "consumerApp": consumer, "providerApp": "tooldata", "entity": "contacts",
        "actions": ["list"], "fields": ["first_name"],
    })).await;
    assert_eq!(status, StatusCode::CREATED, "{grant}");
    let grant_id = grant["id"].as_str().unwrap();
    let (status, approved) = rt.post_json(
        &format!("/api/v1/cross-app/grants/{grant_id}/approve"), &json!({}),
    ).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let (status, body) = rt.put_json(&path, &json!({"graph": graph, "enabled": true})).await;
    assert_eq!(status, StatusCode::OK, "supported graph save: {body}");
    let (status, run) = rt.post_json(&format!("{path}/run"), &json!({})).await;
    assert_eq!(status, StatusCode::OK, "{run}");
    let exec = run["executionId"].as_str().unwrap();
    let terminal = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let status: String = sqlx::query_scalar("SELECT status FROM rootcx_system.workflow_executions WHERE id = $1::uuid")
                .bind(exec).fetch_one(rt.pool()).await.unwrap();
            if matches!(status.as_str(), "succeeded" | "failed" | "canceled") { break status; }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }).await.expect("workflow must finish");
    let (output, error): (Value, Option<String>) = sqlx::query_as(
        "SELECT output, error FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid AND node_id = 'operation'",
    ).bind(exec).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(terminal, "succeeded", "granted query_data must run: {error:?}");
    assert_eq!(output[0][0]["json"]["id"], record["id"], "{output}");
    assert_eq!(output[0][0]["json"]["first_name"], "Ada", "{output}");
    rt.shutdown().await;
}
