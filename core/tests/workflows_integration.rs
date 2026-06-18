mod harness;
use harness::TestRuntime;
use serde_json::{Value, json};

fn simple_graph() -> Value {
    json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "s", "kind": {"type": "control", "control": "set"}, "params": {"fields": {"done": true}}, "position": [1,0]}
        ],
        "edges": [{"from": "t", "to": "s", "fromOutput": 0}]
    })
}

fn if_branch_graph() -> Value {
    json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "setup", "kind": {"type": "control", "control": "set"}, "params": {"fields": {"active": true, "name": "Test"}}, "position": [1,0]},
            {"id": "check", "kind": {"type": "control", "control": "if"}, "params": {"condition": "{{ $json.active }}"}, "position": [2,0]},
            {"id": "yes", "kind": {"type": "control", "control": "set"}, "params": {"fields": {"result": "Hello {{ $json.name }}!"}}, "position": [3,0]},
            {"id": "no", "kind": {"type": "control", "control": "set"}, "params": {"fields": {"result": "inactive"}}, "position": [3,1]}
        ],
        "edges": [
            {"from": "t", "to": "setup", "fromOutput": 0},
            {"from": "setup", "to": "check", "fromOutput": 0},
            {"from": "check", "to": "yes", "fromOutput": 0},
            {"from": "check", "to": "no", "fromOutput": 1}
        ]
    })
}

// ── CRUD ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn workflow_crud_lifecycle() {
    let rt = TestRuntime::boot().await;

    let (s, body) = rt.post_json("/api/v1/workflows", &json!({"name": "wf1", "graph": simple_graph()})).await;
    assert_eq!(s, 201, "create: {body}");
    let wf_id = body["id"].as_str().unwrap();

    let (s, body) = rt.get_json(&format!("/api/v1/workflows/{wf_id}")).await;
    assert_eq!(s, 200);
    assert_eq!(body["name"], "wf1");
    assert_eq!(body["enabled"], false);

    let (s, _) = rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true, "name": "wf1-renamed"})).await;
    assert_eq!(s, 200);

    let (s, body) = rt.get_json(&format!("/api/v1/workflows/{wf_id}")).await;
    assert_eq!(s, 200);
    assert_eq!(body["name"], "wf1-renamed");
    assert_eq!(body["enabled"], true);

    let (s, list) = rt.get_json("/api/v1/workflows").await;
    assert_eq!(s, 200);
    assert!(list.as_array().unwrap().len() >= 1);

    let s = rt.delete(&format!("/api/v1/workflows/{wf_id}")).await;
    assert_eq!(s, 200);

    let (s, _) = rt.get_json(&format!("/api/v1/workflows/{wf_id}")).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn workflow_duplicate_name_rejected() {
    let rt = TestRuntime::boot().await;

    let (s, _) = rt.post_json("/api/v1/workflows", &json!({"name": "dup-test"})).await;
    assert_eq!(s, 201);

    let (s, body) = rt.post_json("/api/v1/workflows", &json!({"name": "dup-test"})).await;
    assert_eq!(s, 400, "duplicate name should be rejected: {body}");
}

// ── Run ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn workflow_run_linear_dag() {
    let rt = TestRuntime::boot().await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "linear-run", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, body) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(body["status"], "succeeded");
    let runs = body["nodeRuns"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|r| r["status"] == "succeeded"));
}

#[tokio::test]
async fn workflow_run_disabled_returns_404() {
    let rt = TestRuntime::boot().await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "disabled-run", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();

    let (s, _) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 404, "disabled workflow should not run");
}

// ── Branching + Expressions ──────────────────────────────────────────

#[tokio::test]
async fn workflow_if_branch_truthy_path() {
    let rt = TestRuntime::boot().await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "if-branch", "graph": if_branch_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, body) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(body["status"], "succeeded");

    let runs = body["nodeRuns"].as_array().unwrap();
    let node_ids: Vec<&str> = runs.iter().map(|r| r["nodeId"].as_str().unwrap()).collect();
    assert!(node_ids.contains(&"yes"), "truthy branch should execute");
    assert!(!node_ids.contains(&"no"), "falsy branch should be skipped");

    let yes_output: Value = sqlx::query_scalar(
        "SELECT output FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid AND node_id = 'yes'",
    ).bind(body["executionId"].as_str().unwrap()).fetch_one(rt.pool()).await.unwrap();
    // Items format: [[{"json": {"result": "Hello Test!"}}]]
    assert_eq!(yes_output[0][0]["json"]["result"], "Hello Test!");
}

// ── Tool batch mode ──────────────────────────────────────────────────

// A read (query_data) downstream of a node that emits N items must run ONCE
// over the whole batch, not once per item. Per-item would re-run the same query
// N times: here the downstream read would yield 2×2 = 4 items instead of 2.
#[tokio::test]
async fn workflow_query_data_runs_once_not_per_item() {
    let rt = TestRuntime::boot().await;
    rt.install("wfbatch", "contacts").await;
    rt.create("wfbatch", "contacts", &json!({"first_name": "A", "last_name": "X"})).await;
    rt.create("wfbatch", "contacts", &json!({"first_name": "B", "last_name": "Y"})).await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "src", "kind": {"type": "tool", "toolName": "query_data"}, "params": {"app": "wfbatch", "entity": "contacts"}, "position": [1,0]},
            {"id": "dst", "kind": {"type": "tool", "toolName": "query_data"}, "params": {"app": "wfbatch", "entity": "contacts"}, "position": [2,0]}
        ],
        "edges": [{"from": "t", "to": "src", "fromOutput": 0}, {"from": "src", "to": "dst", "fromOutput": 0}]
    });

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "batch-once", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, body) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 200, "{body}");
    assert_eq!(body["status"], "succeeded");

    let exec_id = body["executionId"].as_str().unwrap();
    let outputs: Vec<(String, Value)> = sqlx::query_as(
        "SELECT node_id, output FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid",
    ).bind(exec_id).fetch_all(rt.pool()).await.unwrap();
    let port0_len = |node: &str| outputs.iter().find(|(id, _)| id == node)
        .and_then(|(_, o)| o[0].as_array()).map(|a| a.len()).unwrap_or(0);

    assert_eq!(port0_len("src"), 2, "upstream emits 2 items (precondition)");
    assert_eq!(port0_len("dst"), 2, "downstream read runs once, not per item (would be 4)");
}

// ── Palette ──────────────────────────────────────────────────────────

// Palette = { tools, data }. `tools` keeps the generic descriptors (incl. the
// CRUD primitives, so the config panel can resolve their schema); `data`
// enumerates installed (app, entity) pairs the editor turns into CRUD presets.
#[tokio::test]
async fn workflow_node_palette_lists_tools_and_data_entities() {
    let rt = TestRuntime::boot().await;
    rt.install("crm", "contacts").await;

    let (s, body) = rt.get_json("/api/v1/workflows/nodes").await;
    assert_eq!(s, 200);

    let tools = body["tools"].as_array().expect("tools should be an array");
    assert!(
        tools.iter().any(|t| t["name"] == "query_data"),
        "tools must keep query_data so the panel can resolve its schema",
    );
    assert!(tools.iter().all(|t| t.get("inputSchema").is_some()), "descriptors carry inputSchema");

    let data = body["data"].as_array().expect("data should be an array");
    assert!(
        data.iter().any(|d| d["app"] == "crm" && d["entity"] == "contacts"),
        "data must enumerate installed (app, entity): got {data:?}",
    );
}

// ── Executions list ──────────────────────────────────────────────────

#[tokio::test]
async fn workflow_executions_list() {
    let rt = TestRuntime::boot().await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "exec-list", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;

    let (s, body) = rt.get_json(&format!("/api/v1/workflows/{wf_id}/executions")).await;
    assert_eq!(s, 200);
    let execs = body.as_array().unwrap();
    assert_eq!(execs.len(), 2);
    assert!(execs.iter().all(|e| e["status"] == "succeeded"));
}

// ── Cron trigger ─────────────────────────────────────────────────────

#[tokio::test]
async fn workflow_cron_trigger_fires() {
    let rt = TestRuntime::boot().await;
    rt.install("wfcron", "items").await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "cron-trigger", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    // Get the backing app_id for the cron
    let app_id: String = sqlx::query_scalar(
        "SELECT app_id FROM rootcx_system.workflows WHERE id = $1::uuid",
    ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();

    let (s, _) = rt.post_json(&format!("/api/v1/apps/{app_id}/crons"), &json!({
        "name": "wf-trigger",
        "schedule": "1 seconds",
        "payload": {"workflow_id": wf_id}
    })).await;
    assert_eq!(s, 201);

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let (s, body) = rt.get_json(&format!("/api/v1/workflows/{wf_id}/executions")).await;
    assert_eq!(s, 200);
    let execs = body.as_array().unwrap();
    assert!(!execs.is_empty(), "cron should have triggered at least one execution");
    assert_eq!(execs[0]["status"], "succeeded");
}

// ── Record-change (hook) trigger ─────────────────────────────────────

#[tokio::test]
async fn workflow_hook_trigger_fires_on_insert() {
    let rt = TestRuntime::boot().await;
    rt.install("wfhook", "contacts").await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "on-create", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, _) = rt.post_json("/api/v1/apps/wfhook/hooks", &json!({
        "entity": "contacts",
        "operation": "INSERT",
        "action_type": "workflow",
        "action_config": {"workflow_id": wf_id}
    })).await;
    assert!(s.is_success(), "hook creation should succeed (got {s})");

    rt.create("wfhook", "contacts", &json!({
        "first_name": "Jane", "last_name": "Doe"
    })).await;

    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    let (s, body) = rt.get_json(&format!("/api/v1/workflows/{wf_id}/executions")).await;
    assert_eq!(s, 200);
    let execs = body.as_array().unwrap();
    assert!(!execs.is_empty(), "hook should have triggered workflow on INSERT");
    assert_eq!(execs[0]["status"], "succeeded");
}

// ── Auth boundary ────────────────────────────────────────────────────

#[tokio::test]
async fn workflow_endpoints_reject_unauthenticated() {
    let rt = TestRuntime::boot().await;

    let s = rt.get_unauthed("/api/v1/workflows").await;
    assert_eq!(s, 401);

    let (s, _) = rt.post_unauthed("/api/v1/workflows", &json!({"name": "x"})).await;
    assert_eq!(s, 401);
}
