mod harness;
use harness::TestRuntime;
use reqwest::Method;
use serde_json::{Value, json};

/// Runs now go through pgmq (async); poll the execution row to a terminal state.
async fn wait_exec(rt: &TestRuntime, exec_id: &str) -> String {
    for _ in 0..200 {
        let st: Option<String> = sqlx::query_scalar(
            "SELECT status FROM rootcx_system.workflow_executions WHERE id = $1::uuid",
        ).bind(exec_id).fetch_optional(rt.pool()).await.unwrap();
        if let Some(s) = st {
            if matches!(s.as_str(), "succeeded" | "failed" | "canceled") { return s; }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("execution {exec_id} did not finish in time");
}

async fn node_status(rt: &TestRuntime, exec_id: &str, node_id: &str) -> Option<(String, i32, Option<String>)> {
    sqlx::query_as(
        "SELECT status, attempts, error FROM rootcx_system.workflow_node_runs
         WHERE execution_id = $1::uuid AND node_id = $2",
    ).bind(exec_id).bind(node_id).fetch_optional(rt.pool()).await.unwrap()
}

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
    let exec = body["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "succeeded");

    let succeeded: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid AND status = 'succeeded'",
    ).bind(exec).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(succeeded, 2);
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
    let exec = body["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "succeeded");

    assert!(node_status(&rt, exec, "yes").await.is_some(), "truthy branch should execute");
    assert!(node_status(&rt, exec, "no").await.is_none(), "falsy branch should be skipped");

    let yes_output: Value = sqlx::query_scalar(
        "SELECT output FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid AND node_id = 'yes'",
    ).bind(exec).fetch_one(rt.pool()).await.unwrap();
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
    let exec_id = body["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec_id).await, "succeeded");

    let outputs: Vec<(String, Value)> = sqlx::query_as(
        "SELECT node_id, output FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid",
    ).bind(exec_id).fetch_all(rt.pool()).await.unwrap();
    let port0_len = |node: &str| outputs.iter().find(|(id, _)| id == node)
        .and_then(|(_, o)| o[0].as_array()).map(|a| a.len()).unwrap_or(0);

    assert_eq!(port0_len("src"), 2, "upstream emits 2 items (precondition)");
    assert_eq!(port0_len("dst"), 2, "downstream read runs once, not per item (would be 4)");
}

// ── Durable runner: retry + continue-on-error ────────────────────────

// A node configured with retry exhausts its attempts on persistent failure, the
// attempt count is persisted, and the execution ends 'failed'. (query_data on a
// non-existent entity is a deterministic failure.)
#[tokio::test]
async fn workflow_node_retry_exhausts_then_fails() {
    let rt = TestRuntime::boot().await;
    rt.install("wfretry", "contacts").await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "bad", "kind": {"type": "tool", "toolName": "query_data"},
             "params": {"app": "wfretry", "entity": "ghosts", "retry": {"maxAttempts": 2, "backoffMs": 1}}, "position": [1,0]}
        ],
        "edges": [{"from": "t", "to": "bad", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "retry-wf", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, body) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 200, "{body}");
    let exec = body["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "failed");

    let (status, attempts, _) = node_status(&rt, exec, "bad").await.expect("bad node ran");
    assert_eq!(status, "failed");
    assert_eq!(attempts, 2, "retried up to maxAttempts");
}

// continueOnError lets a failed node route an error item downstream instead of
// aborting: the downstream node runs and the execution ends 'succeeded'.
#[tokio::test]
async fn workflow_continue_on_error_proceeds_downstream() {
    let rt = TestRuntime::boot().await;
    rt.install("wfcoe", "contacts").await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "bad", "kind": {"type": "tool", "toolName": "query_data"},
             "params": {"app": "wfcoe", "entity": "ghosts", "continueOnError": true}, "position": [1,0]},
            {"id": "after", "kind": {"type": "control", "control": "set"},
             "params": {"fields": {"recovered": true}}, "position": [2,0]}
        ],
        "edges": [{"from": "t", "to": "bad", "fromOutput": 0}, {"from": "bad", "to": "after", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "coe-wf", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (s, body) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(s, 200, "{body}");
    let exec = body["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "succeeded", "continueOnError must not abort the run");

    let after = node_status(&rt, exec, "after").await.expect("downstream node ran");
    assert_eq!(after.0, "succeeded", "downstream node ran after the recovered failure");

    // The failed node carries its error even though the run continued.
    let bad = node_status(&rt, exec, "bad").await.expect("bad node recorded");
    assert!(bad.2.is_some(), "error preserved for debugging");
}

// A Stop node ends the run gracefully (succeeded) and halts everything after it.
#[tokio::test]
async fn workflow_stop_node_succeeds_and_halts() {
    let rt = TestRuntime::boot().await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "stop", "kind": {"type": "control", "control": "stop"}, "params": {"message": "done here"}, "position": [1,0]},
            {"id": "after", "kind": {"type": "control", "control": "set"}, "params": {"fields": {"x": 1}}, "position": [2,0]}
        ],
        "edges": [{"from": "t", "to": "stop", "fromOutput": 0}, {"from": "stop", "to": "after", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "stop-wf", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (_, run) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    let exec = run["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "succeeded", "Stop ends the run as succeeded");

    assert_eq!(node_status(&rt, exec, "stop").await.expect("stop ran").0, "succeeded");
    assert!(node_status(&rt, exec, "after").await.is_none(), "nodes after Stop are halted");
}

// A create node sets a deterministic id derived from (execution, node, item), so a
// crash-resume re-insert hits ON CONFLICT instead of duplicating the row. Verifies
// the idempotency key is threaded end-to-end into mutate_data (a broken thread →
// random id → this fails).
#[tokio::test]
async fn workflow_create_node_id_is_idempotent() {
    let rt = TestRuntime::boot().await;
    rt.install("wfidem", "contacts").await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "make", "kind": {"type": "tool", "toolName": "mutate_data"},
             "params": {"app": "wfidem", "entity": "contacts", "action": "create", "data": {"first_name": "Id", "last_name": "Em"}}, "position": [1,0]}
        ],
        "edges": [{"from": "t", "to": "make", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "idem-wf", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;
    let (_, run) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    let exec = run["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "succeeded");

    let ids: Vec<sqlx::types::Uuid> = sqlx::query_scalar(r#"SELECT id FROM "wfidem"."contacts""#)
        .fetch_all(rt.pool()).await.unwrap();
    assert_eq!(ids.len(), 1, "exactly one row created");
    let expected = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, format!("{exec}:make:0").as_bytes());
    assert_eq!(ids[0], expected, "row id is the deterministic idempotency uuid (resume-safe)");
}

// bulk_create can't be made resume-safe (one multi-row INSERT), so it's refused on
// the durable workflow path — the run fails loudly and writes nothing, rather than
// risking a duplicated batch on resume.
#[tokio::test]
async fn workflow_bulk_create_refused_on_durable_path() {
    let rt = TestRuntime::boot().await;
    rt.install("wfbulk", "contacts").await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "bulk", "kind": {"type": "tool", "toolName": "mutate_data"},
             "params": {"app": "wfbulk", "entity": "contacts", "action": "bulk_create",
                        "data": [{"first_name": "A", "last_name": "X"}, {"first_name": "B", "last_name": "Y"}]}, "position": [1,0]}
        ],
        "edges": [{"from": "t", "to": "bulk", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "bulk-wf", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;
    let (_, run) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    let exec = run["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "failed");

    let (_, _, err) = node_status(&rt, exec, "bulk").await.expect("bulk node ran");
    assert!(err.unwrap_or_default().contains("not allowed inside a workflow"), "refused with guidance");
    let n: i64 = sqlx::query_scalar(r#"SELECT count(*) FROM "wfbulk"."contacts""#).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(n, 0, "no rows written");
}

// Expressions in tool node params resolve per-item (not globally over the batch).
// A bug here would produce `undefined` or the wrong value in the created records.
#[tokio::test]
async fn workflow_tool_node_expressions_resolve_per_item() {
    let rt = TestRuntime::boot().await;
    rt.install("wfexpr", "contacts").await;
    rt.create("wfexpr", "contacts", &json!({"first_name": "Alice", "last_name": "A"})).await;
    rt.create("wfexpr", "contacts", &json!({"first_name": "Bob", "last_name": "B"})).await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "manual"}, "params": {}, "position": [0,0]},
            {"id": "fetch", "kind": {"type": "tool", "toolName": "query_data"}, "params": {"app": "wfexpr", "entity": "contacts"}, "position": [1,0]},
            {"id": "tag", "kind": {"type": "tool", "toolName": "mutate_data"},
             "params": {"app": "wfexpr", "entity": "contacts", "action": "update",
                        "id": "{{ $json.id }}", "data": {"notes": "{{ $json.first_name + ' tagged' }}"}},
             "position": [2,0]}
        ],
        "edges": [{"from": "t", "to": "fetch", "fromOutput": 0}, {"from": "fetch", "to": "tag", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "expr-per-item", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;
    let (_, run) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(wait_exec(&rt, run["executionId"].as_str().unwrap()).await, "succeeded");

    let notes: Vec<String> = sqlx::query_scalar(
        r#"SELECT notes FROM "wfexpr"."contacts" WHERE notes IS NOT NULL ORDER BY notes"#,
    ).fetch_all(rt.pool()).await.unwrap();
    assert_eq!(notes, vec!["Alice tagged", "Bob tagged"], "each item resolved its own $json.first_name");
}

// ── SSE progress stream ──────────────────────────────────────────────

// The editor consumes run progress over SSE. The stream replays persisted
// node_runs then forwards live events, and closes on the terminal `done` — so
// reading the whole body yields the full event log regardless of connect timing.
#[tokio::test]
async fn workflow_run_streams_progress_over_sse() {
    let rt = TestRuntime::boot().await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "sse-wf", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (_, run) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    let exec = run["executionId"].as_str().unwrap();

    let url = rt.url(&format!("/api/v1/workflows/{wf_id}/executions/{exec}/stream"));
    let resp = rt.client.get(&url).bearer_auth(&rt.token).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    // Stream closes on `done`, so the body is the complete event log.
    let text = tokio::time::timeout(std::time::Duration::from_secs(20), resp.text())
        .await.expect("sse stream did not close").unwrap();

    assert!(text.contains("event: node"), "should stream per-node events: {text}");
    assert!(text.contains("event: done"), "should end with a terminal event: {text}");
    assert!(text.contains("\"status\":\"succeeded\""), "final status in stream: {text}");
}

// Attaching AFTER a run has finished must still yield the full log: this drives
// the DB-replay branch (distinct from the live broadcast above) — the path that
// reloads node_runs and closes the per-exec channel. Guards against a reconnect
// returning an empty stream.
#[tokio::test]
async fn workflow_stream_replays_a_finished_run() {
    let rt = TestRuntime::boot().await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "sse-replay", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    let (_, run) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    let exec = run["executionId"].as_str().unwrap();
    assert_eq!(wait_exec(&rt, exec).await, "succeeded"); // finish before attaching

    let url = rt.url(&format!("/api/v1/workflows/{wf_id}/executions/{exec}/stream"));
    let resp = rt.client.get(&url).bearer_auth(&rt.token).send().await.unwrap();
    let text = tokio::time::timeout(std::time::Duration::from_secs(20), resp.text())
        .await.expect("sse stream did not close").unwrap();

    assert!(text.contains("event: node"), "replays persisted node runs: {text}");
    assert!(text.contains("event: done"), "ends with the terminal event: {text}");
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

    let (_, r1) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    let (_, r2) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    assert_eq!(wait_exec(&rt, r1["executionId"].as_str().unwrap()).await, "succeeded");
    assert_eq!(wait_exec(&rt, r2["executionId"].as_str().unwrap()).await, "succeeded");

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

// Record-change hooks are auto-managed: enable creates the hook from the trigger
// node's params, disable removes it, and the hook actually fires on record insert.
#[tokio::test]
async fn workflow_record_change_hook_auto_lifecycle() {
    let rt = TestRuntime::boot().await;
    rt.install("wfhookrc", "contacts").await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "recordChange"},
             "params": {"app": "wfhookrc", "entity": "contacts", "operation": "INSERT"}, "position": [0,0]},
            {"id": "s", "kind": {"type": "control", "control": "set"},
             "params": {"fields": {"saw": "{{ $json.record ? $json.record.first_name : 'none' }}"}}, "position": [1,0]}
        ],
        "edges": [{"from": "t", "to": "s", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "rc-auto", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();

    // Not yet enabled → no hook exists.
    let hooks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.entity_hooks WHERE action_config->>'workflow_id' = $1",
    ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(hooks, 0, "no hook before enable");

    // Enable → hook auto-created.
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;
    let hooks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.entity_hooks WHERE action_config->>'workflow_id' = $1",
    ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(hooks, 1, "hook created on enable");

    // Insert a record → workflow fires.
    rt.create("wfhookrc", "contacts", &json!({"first_name": "Auto", "last_name": "Hook"})).await;
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    let status: String = sqlx::query_scalar(
        "SELECT status FROM rootcx_system.workflow_executions WHERE workflow_id = $1::uuid ORDER BY created_at DESC LIMIT 1",
    ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(status, "succeeded", "record change fired the workflow");

    // Disable → hook removed.
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": false})).await;
    let hooks: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rootcx_system.entity_hooks WHERE action_config->>'workflow_id' = $1",
    ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(hooks, 0, "hook removed on disable");
}

// ── Webhook trigger ─────────────────────────────────────────────────

// The webhook endpoint is public (no auth): a POST triggers the workflow run-as-owner
// and the body arrives as $json in the trigger node (accessible by downstream nodes).
#[tokio::test]
async fn workflow_webhook_trigger_fires_and_passes_body() {
    let rt = TestRuntime::boot().await;

    let graph = json!({
        "nodes": [
            {"id": "t", "kind": {"type": "trigger", "trigger": "webhook"}, "params": {}, "position": [0,0]},
            {"id": "echo", "kind": {"type": "control", "control": "set"},
             "params": {"fields": {"got": "{{ $json.record ? $json.record.ping : 'none' }}"}}, "position": [1,0]}
        ],
        "edges": [{"from": "t", "to": "echo", "fromOutput": 0}]
    });
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "webhook-test", "graph": graph})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;

    // POST without auth (public endpoint)
    let (s, resp) = rt.post_unauthed(
        &format!("/api/v1/workflows/{wf_id}/webhook"),
        &json!({"ping": "pong"}),
    ).await;
    assert_eq!(s, 200, "webhook must accept without auth: {resp}");
    assert_eq!(resp["status"], "accepted");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

    let exec_id: String = sqlx::query_scalar(
        "SELECT id::text FROM rootcx_system.workflow_executions WHERE workflow_id = $1::uuid ORDER BY created_at DESC LIMIT 1",
    ).bind(wf_id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(wait_exec(&rt, &exec_id).await, "succeeded");

    // The Set node received the webhook body via trigger_data.
    let output: Value = sqlx::query_scalar(
        "SELECT output FROM rootcx_system.workflow_node_runs WHERE execution_id = $1::uuid AND node_id = 'echo'",
    ).bind(&exec_id).fetch_one(rt.pool()).await.unwrap();
    assert_eq!(output[0][0]["json"]["got"], "pong", "webhook body propagated to downstream node");
}

// A disabled workflow's webhook returns 404 (not 500, not silently queued).
#[tokio::test]
async fn workflow_webhook_rejects_disabled() {
    let rt = TestRuntime::boot().await;

    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "wh-disabled", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    // NOT enabled

    let (s, _) = rt.post_unauthed(&format!("/api/v1/workflows/{wf_id}/webhook"), &json!({})).await;
    assert_eq!(s, 404, "disabled workflow webhook must return 404");
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

// Authorization is owner-scoped: a different authenticated user must not read a
// workflow, list its runs, or STREAM run output (which carries node data). Closes
// the horizontal IDOR — being authenticated is not enough.
#[tokio::test]
async fn workflow_access_is_owner_scoped() {
    let rt = TestRuntime::boot().await;

    // Admin (the default identity) owns the workflow and runs it.
    let (_, body) = rt.post_json("/api/v1/workflows", &json!({"name": "owner-only", "graph": simple_graph()})).await;
    let wf_id = body["id"].as_str().unwrap();
    rt.put_json(&format!("/api/v1/workflows/{wf_id}"), &json!({"enabled": true})).await;
    let (_, run) = rt.post_json(&format!("/api/v1/workflows/{wf_id}/run"), &json!({})).await;
    let exec = run["executionId"].as_str().unwrap();

    // A separate, non-admin user is denied (NotFound, so existence isn't leaked).
    let intruder = rt.register_and_login("intruder@test.local").await;
    for path in [
        format!("/api/v1/workflows/{wf_id}"),
        format!("/api/v1/workflows/{wf_id}/executions"),
        format!("/api/v1/workflows/{wf_id}/executions/{exec}/stream"),
    ] {
        let (s, _) = rt.request_as(Method::GET, &path, &intruder, None).await;
        assert_eq!(s, 404, "non-owner must not access {path}");
    }

    // The owner still has access.
    let (s, _) = rt.get_json(&format!("/api/v1/workflows/{wf_id}/executions")).await;
    assert_eq!(s, 200, "owner retains access");
}
