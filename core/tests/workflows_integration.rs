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
    rt.install("wfapp", "items").await;

    let (s, body) = rt.post_json("/api/v1/apps/wfapp/workflows", &json!({"name": "wf1", "graph": simple_graph()})).await;
    assert_eq!(s, 201, "create: {body}");
    let wf_id = body["id"].as_str().unwrap();

    let (s, body) = rt.get_json(&format!("/api/v1/apps/wfapp/workflows/{wf_id}")).await;
    assert_eq!(s, 200);
    assert_eq!(body["name"], "wf1");
    assert_eq!(body["enabled"], false);

    let (s, _) = rt.put_json(&format!("/api/v1/apps/wfapp/workflows/{wf_id}"), &json!({"enabled": true, "name": "wf1-renamed"})).await;
    assert_eq!(s, 200);

    let (s, body) = rt.get_json(&format!("/api/v1/apps/wfapp/workflows/{wf_id}")).await;
    assert_eq!(s, 200);
    assert_eq!(body["name"], "wf1-renamed");
    assert_eq!(body["enabled"], true);

    let (s, list) = rt.get_json("/api/v1/apps/wfapp/workflows").await;
    assert_eq!(s, 200);
    assert_eq!(list.as_array().unwrap().len(), 1);

    let s = rt.delete(&format!("/api/v1/apps/wfapp/workflows/{wf_id}")).await;
    assert_eq!(s, 200);

    let (s, _) = rt.get_json(&format!("/api/v1/apps/wfapp/workflows/{wf_id}")).await;
    assert_eq!(s, 404);
}

#[tokio::test]
async fn workflow_duplicate_name_rejected() {
    let rt = TestRuntime::boot().await;
    rt.install("wfdup", "items").await;

    let (s, _) = rt.post_json("/api/v1/apps/wfdup/workflows", &json!({"name": "dup"})).await;
    assert_eq!(s, 201);

    let (s, body) = rt.post_json("/api/v1/apps/wfdup/workflows", &json!({"name": "dup"})).await;
    assert_eq!(s, 400, "duplicate name should be rejected: {body}");
}

// ── Run ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn workflow_run_linear_dag() {
    let rt = TestRuntime::boot().await;
    rt.install("wfrun", "items").await;

    let (_, body) = rt.post_json("/api/v1/apps/wfrun/workflows", &json!({"name": "linear", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/apps/wfrun/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, body) = rt.post_json(&format!("/api/v1/apps/wfrun/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(body["status"], "succeeded");
    let runs = body["nodeRuns"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0]["nodeId"], "t");
    assert_eq!(runs[1]["nodeId"], "s");
    assert!(runs.iter().all(|r| r["status"] == "succeeded"));
}

#[tokio::test]
async fn workflow_run_disabled_returns_404() {
    let rt = TestRuntime::boot().await;
    rt.install("wfdis", "items").await;

    let (_, body) = rt.post_json("/api/v1/apps/wfdis/workflows", &json!({"name": "off", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();

    let (s, _) = rt.post_json(&format!("/api/v1/apps/wfdis/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 404, "disabled workflow should not run");
}

// ── Branching + Expressions ──────────────────────────────────────────

#[tokio::test]
async fn workflow_if_branch_truthy_path() {
    let rt = TestRuntime::boot().await;
    rt.install("wfif", "items").await;

    let (_, body) = rt.post_json("/api/v1/apps/wfif/workflows", &json!({"name": "branch", "graph": if_branch_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/apps/wfif/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, body) = rt.post_json(&format!("/api/v1/apps/wfif/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 200);
    assert_eq!(body["status"], "succeeded");

    let runs = body["nodeRuns"].as_array().unwrap();
    let node_ids: Vec<&str> = runs.iter().map(|r| r["nodeId"].as_str().unwrap()).collect();
    assert!(node_ids.contains(&"yes"), "truthy branch should execute");
    assert!(!node_ids.contains(&"no"), "falsy branch should be skipped");

    // Verify expression resolved in the output
    let yes_output: Value = sqlx::query_scalar(
        "SELECT output FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid AND node_id = 'yes'",
    ).bind(body["executionId"].as_str().unwrap()).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(yes_output["result"], "Hello Test!", "expression {{ $json.name }} should resolve");
}

// ── Palette ──────────────────────────────────────────────────────────

#[tokio::test]
async fn workflow_node_palette_returns_tools() {
    let rt = TestRuntime::boot().await;
    rt.install("wfpal", "items").await;

    let (s, body) = rt.get_json("/api/v1/apps/wfpal/nodes").await;
    assert_eq!(s, 200);
    let nodes = body.as_array().unwrap();
    assert!(!nodes.is_empty(), "palette should return at least one tool");
    assert!(nodes[0].get("name").is_some(), "each node should have a name");
    assert!(nodes[0].get("inputSchema").is_some(), "each node should have inputSchema");
}

// ── Executions list ──────────────────────────────────────────────────

#[tokio::test]
async fn workflow_executions_list() {
    let rt = TestRuntime::boot().await;
    rt.install("wfexec", "items").await;

    let (_, body) = rt.post_json("/api/v1/apps/wfexec/workflows", &json!({"name": "ex", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/apps/wfexec/workflows/{wf_id}"), &json!({"enabled": true})).await;

    rt.post_json(&format!("/api/v1/apps/wfexec/workflows/{wf_id}/run"), &json!({})).await;
    rt.post_json(&format!("/api/v1/apps/wfexec/workflows/{wf_id}/run"), &json!({})).await;

    let (s, body) = rt.get_json(&format!("/api/v1/apps/wfexec/workflows/{wf_id}/executions")).await;
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

    let (_, body) = rt.post_json("/api/v1/apps/wfcron/workflows", &json!({"name": "scheduled", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/apps/wfcron/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, _) = rt.post_json("/api/v1/apps/wfcron/crons", &json!({
        "name": "wf-trigger",
        "schedule": "1 seconds",
        "payload": {"workflow_id": wf_id}
    })).await;
    assert_eq!(s, 201);

    // Wait for the cron to fire (pg_cron runs on 1-second intervals + scheduler 500ms poll)
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let (s, body) = rt.get_json(&format!("/api/v1/apps/wfcron/workflows/{wf_id}/executions")).await;
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

    let (_, body) = rt.post_json("/api/v1/apps/wfhook/workflows", &json!({"name": "on-create", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/apps/wfhook/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, _) = rt.post_json("/api/v1/apps/wfhook/hooks", &json!({
        "entity": "contacts",
        "operation": "INSERT",
        "action_type": "workflow",
        "action_config": {"workflow_id": wf_id}
    })).await;
    assert!(s.is_success(), "hook creation should succeed (got {s})");

    // Insert a record to trigger the hook
    rt.create("wfhook", "contacts", &json!({
        "first_name": "Jane", "last_name": "Doe"
    })).await;

    // Wait for hook -> pgmq -> scheduler -> workflow execution
    // Container + scheduler poll (500ms) + workflow execution needs generous margin
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    let (s, body) = rt.get_json(&format!("/api/v1/apps/wfhook/workflows/{wf_id}/executions")).await;
    assert_eq!(s, 200);
    let execs = body.as_array().unwrap();
    assert!(!execs.is_empty(), "hook should have triggered workflow on INSERT");
    assert_eq!(execs[0]["status"], "succeeded");
}

// ── Auth boundary ────────────────────────────────────────────────────

#[tokio::test]
async fn workflow_endpoints_reject_unauthenticated() {
    let rt = TestRuntime::boot().await;
    rt.install("wfauth", "items").await;

    let s = rt.get_unauthed("/api/v1/apps/wfauth/workflows").await;
    assert_eq!(s, 401);

    let (s, _) = rt.post_unauthed("/api/v1/apps/wfauth/workflows", &json!({"name": "x"})).await;
    assert_eq!(s, 401);
}
