use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::tools::{DispatchError, ToolContext, ToolRegistry};
use rootcx_types::{ControlKind, WorkflowGraph, WorkflowNodeKind, WorkflowNodeRunStatus, WorkflowExecutionStatus};

use super::expr;

pub struct NodeRunResult {
    pub node_id: String,
    pub status: WorkflowNodeRunStatus,
    pub error: Option<String>,
}

pub async fn execute_dag(
    registry: &Arc<ToolRegistry>,
    pool: &sqlx::PgPool,
    app_id: &str,
    user_id: Uuid,
    perms: &[String],
    graph: &WorkflowGraph,
    exec_id: Uuid,
) -> Vec<NodeRunResult> {
    let node_map: HashMap<&str, &rootcx_types::WorkflowNode> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let order = topo_sort(graph);
    let mut outputs: HashMap<String, JsonValue> = HashMap::new();
    // Tracks which output branch each node selected (for If/Switch routing)
    let mut branch_outputs: HashMap<String, u8> = HashMap::new();
    let mut results = Vec::new();

    for node_id in &order {
        let Some(node) = node_map.get(node_id.as_str()) else { continue };

        if !should_execute(node_id, graph, &outputs, &branch_outputs) {
            continue;
        }

        let input = collect_inputs(node_id, graph, &outputs);
        let resolved_params = expr::resolve(&node.params, &input, &outputs);

        let (status, output, error) = match &node.kind {
            WorkflowNodeKind::Trigger { .. } => {
                (WorkflowNodeRunStatus::Succeeded, json!({}), None)
            }
            WorkflowNodeKind::Tool { tool_name } => {
                execute_tool_node(registry, pool, app_id, user_id, perms, tool_name, &resolved_params, &input).await
            }
            WorkflowNodeKind::Control { control } => {
                execute_control_node(control, &resolved_params, &input)
            }
            _ => (WorkflowNodeRunStatus::Skipped, json!(null), Some("not implemented".into())),
        };

        sqlx::query(
            "INSERT INTO rootcx_system.workflow_node_runs (execution_id, node_id, status, input, output, error, started_at, finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, now(), now())",
        ).bind(exec_id).bind(node_id).bind(status.as_str())
        .bind(&input).bind(&output).bind(&error)
        .execute(pool).await.ok();

        if status == WorkflowNodeRunStatus::Succeeded {
            if let Some(branch) = output.get("_branch").and_then(|b| b.as_u64()) {
                branch_outputs.insert(node_id.clone(), branch as u8);
            }
            outputs.insert(node_id.clone(), output);
        }
        results.push(NodeRunResult { node_id: node_id.clone(), status, error });

        if status == WorkflowNodeRunStatus::Failed { break; }
    }

    results
}

/// Full lifecycle: create execution, run DAG, finalize status. Shared by
/// the HTTP run endpoint and the scheduler trigger path.
pub async fn run_workflow(
    registry: &Arc<ToolRegistry>,
    pool: &PgPool,
    app_id: &str,
    workflow_id: Uuid,
    user_id: Uuid,
    perms: &[String],
    trigger_data: Option<JsonValue>,
) -> Result<(Uuid, Vec<NodeRunResult>), String> {
    let graph_json: JsonValue = sqlx::query_scalar(
        "SELECT graph FROM rootcx_system.workflows WHERE id = $1 AND app_id = $2 AND enabled = true",
    ).bind(workflow_id).bind(app_id)
    .fetch_optional(pool).await.map_err(|e| e.to_string())?
    .ok_or_else(|| "workflow not found or not enabled".to_string())?;

    let graph: WorkflowGraph = serde_json::from_value(graph_json)
        .map_err(|e| format!("invalid graph: {e}"))?;

    let exec_id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.workflow_executions (id, workflow_id, app_id, status, run_as_user_id, trigger_data, started_at)
         VALUES (gen_random_uuid(), $1, $2, 'running', $3, $4, now()) RETURNING id",
    ).bind(workflow_id).bind(app_id).bind(user_id).bind(&trigger_data)
    .fetch_one(pool).await.map_err(|e| e.to_string())?;

    let results = execute_dag(registry, pool, app_id, user_id, perms, &graph, exec_id).await;

    let all_ok = results.iter().all(|r| r.status == WorkflowNodeRunStatus::Succeeded);
    let final_status = if all_ok { WorkflowExecutionStatus::Succeeded } else { WorkflowExecutionStatus::Failed };
    let error_msg = results.iter().find_map(|r| {
        if r.status == WorkflowNodeRunStatus::Failed { r.error.clone() } else { None }
    });

    let _ = sqlx::query(
        "UPDATE rootcx_system.workflow_executions SET status = $2, error = $3, finished_at = now() WHERE id = $1",
    ).bind(exec_id).bind(final_status.as_str()).bind(&error_msg)
    .execute(pool).await;

    Ok((exec_id, results))
}

fn should_execute(node_id: &str, graph: &WorkflowGraph, outputs: &HashMap<String, JsonValue>, branches: &HashMap<String, u8>) -> bool {
    let mut inbound = graph.edges.iter().filter(|e| e.to == node_id).peekable();
    if inbound.peek().is_none() { return true; }
    inbound.any(|edge| {
        outputs.contains_key(&edge.from)
            && branches.get(&edge.from).map_or(true, |&chosen| edge.from_output == chosen)
    })
}

// ── Control nodes ────────────────────────────────────────────────────

fn execute_control_node(
    control: &ControlKind,
    params: &JsonValue,
    input: &JsonValue,
) -> (WorkflowNodeRunStatus, JsonValue, Option<String>) {
    match control {
        ControlKind::If => execute_if(params, input),
        ControlKind::Switch => execute_switch(params, input),
        ControlKind::Set => execute_set(params),
        ControlKind::Merge => (WorkflowNodeRunStatus::Succeeded, input.clone(), None),
        ControlKind::Stop => {
            let msg = params.get("message").and_then(|v| v.as_str()).unwrap_or("workflow stopped");
            (WorkflowNodeRunStatus::Failed, json!(null), Some(msg.into()))
        }
        ControlKind::Loop | ControlKind::Wait => {
            (WorkflowNodeRunStatus::Skipped, json!(null), Some("not implemented".into()))
        }
    }
}

fn with_branch(input: &JsonValue, branch: u8) -> JsonValue {
    let mut out = input.clone();
    if let Some(obj) = out.as_object_mut() {
        obj.insert("_branch".into(), json!(branch));
    } else {
        out = json!({ "_branch": branch });
    }
    out
}

fn execute_if(params: &JsonValue, input: &JsonValue) -> (WorkflowNodeRunStatus, JsonValue, Option<String>) {
    let condition = params.get("condition").unwrap_or(&JsonValue::Null);
    let branch: u8 = if is_truthy(condition) { 0 } else { 1 };
    (WorkflowNodeRunStatus::Succeeded, with_branch(input, branch), None)
}

fn execute_switch(params: &JsonValue, input: &JsonValue) -> (WorkflowNodeRunStatus, JsonValue, Option<String>) {
    let value = params.get("value").unwrap_or(&JsonValue::Null);
    let cases = params.get("cases").and_then(|c| c.as_array());
    let branch: u8 = match cases {
        Some(arr) => arr.iter().position(|c| c == value).map(|i| i as u8).unwrap_or(arr.len() as u8),
        None => 0,
    };
    (WorkflowNodeRunStatus::Succeeded, with_branch(input, branch), None)
}

/// Set node: merges `fields` (object) into the output, overwriting input.
fn execute_set(params: &JsonValue) -> (WorkflowNodeRunStatus, JsonValue, Option<String>) {
    let fields = params.get("fields").cloned().unwrap_or(json!({}));
    (WorkflowNodeRunStatus::Succeeded, fields, None)
}

fn is_truthy(value: &JsonValue) -> bool {
    match value {
        JsonValue::Null => false,
        JsonValue::Bool(b) => *b,
        JsonValue::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        JsonValue::String(s) => !s.is_empty() && s != "false" && s != "0",
        JsonValue::Array(a) => !a.is_empty(),
        JsonValue::Object(o) => !o.is_empty(),
    }
}

// ── Tool node ────────────────────────────────────────────────────────

async fn execute_tool_node(
    registry: &Arc<ToolRegistry>,
    pool: &sqlx::PgPool,
    app_id: &str,
    user_id: Uuid,
    perms: &[String],
    tool_name: &str,
    params: &JsonValue,
    input: &JsonValue,
) -> (WorkflowNodeRunStatus, JsonValue, Option<String>) {
    let tool = match registry.get(tool_name) {
        Some(t) => t,
        None => return (WorkflowNodeRunStatus::Failed, json!(null), Some(format!("unknown tool: {tool_name}"))),
    };

    let args = merge_args(params, input);

    let ctx = ToolContext {
        pool: pool.clone(),
        app_id: app_id.into(),
        user_id,
        invoker_user_id: Some(user_id),
        permissions: perms.to_vec(),
        task_scope: None,
        args,
        agent_dispatch: None,
        integration_caller: None,
        action_caller: None,
        stream_tx: None,
    };

    let outcome = crate::tools::dispatch(tool_name, tool, &ctx).await;
    match outcome.value {
        Ok(v) => (WorkflowNodeRunStatus::Succeeded, v, None),
        Err(DispatchError::PermissionDenied(e)) => (WorkflowNodeRunStatus::Failed, json!(null), Some(e)),
        Err(DispatchError::ExecutionFailed(e)) => (WorkflowNodeRunStatus::Failed, json!(null), Some(e)),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

pub(crate) fn merge_args(params: &JsonValue, input: &JsonValue) -> JsonValue {
    let mut base = params.clone();
    if let (Some(b), Some(i)) = (base.as_object_mut(), input.as_object()) {
        for (k, v) in i { b.entry(k.clone()).or_insert_with(|| v.clone()); }
    }
    base
}

pub(crate) fn collect_inputs(node_id: &str, graph: &WorkflowGraph, outputs: &HashMap<String, JsonValue>) -> JsonValue {
    let mut merged = serde_json::Map::new();
    for edge in &graph.edges {
        if edge.to == node_id {
            if let Some(out) = outputs.get(&edge.from) {
                if let Some(obj) = out.as_object() {
                    for (k, v) in obj {
                        if k == "_branch" { continue; }
                        merged.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
        }
    }
    JsonValue::Object(merged)
}

pub(crate) fn topo_sort(graph: &WorkflowGraph) -> Vec<String> {
    use std::collections::VecDeque;
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for node in &graph.nodes { in_degree.entry(&node.id).or_insert(0); }
    for edge in &graph.edges {
        *in_degree.entry(&edge.to).or_insert(0) += 1;
    }
    let mut queue: VecDeque<&str> = in_degree.iter()
        .filter(|(_, d)| **d == 0).map(|(n, _)| *n).collect();
    let mut order = Vec::new();
    while let Some(n) = queue.pop_front() {
        order.push(n.to_string());
        for edge in &graph.edges {
            if edge.from == n {
                if let Some(d) = in_degree.get_mut(edge.to.as_str()) {
                    *d -= 1;
                    if *d == 0 { queue.push_back(&edge.to); }
                }
            }
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use rootcx_types::{WorkflowGraph, WorkflowNode, WorkflowEdge, WorkflowNodeKind, TriggerKind, ControlKind};

    fn trigger(id: &str) -> WorkflowNode {
        WorkflowNode { id: id.into(), kind: WorkflowNodeKind::Trigger { trigger: TriggerKind::Manual }, params: json!({}), position: [0.0, 0.0] }
    }
    fn control(id: &str, kind: ControlKind, params: JsonValue) -> WorkflowNode {
        WorkflowNode { id: id.into(), kind: WorkflowNodeKind::Control { control: kind }, params, position: [0.0, 0.0] }
    }
    fn edge(from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge { from: from.into(), to: to.into(), from_output: 0 }
    }
    fn edge_branch(from: &str, to: &str, branch: u8) -> WorkflowEdge {
        WorkflowEdge { from: from.into(), to: to.into(), from_output: branch }
    }

    #[test]
    fn topo_sort_linear_and_branching() {
        let cases: Vec<(&str, WorkflowGraph, Vec<Vec<&str>>)> = vec![
            ("linear A->B->C", WorkflowGraph {
                nodes: vec![trigger("A"), trigger("B"), trigger("C")],
                edges: vec![edge("A", "B"), edge("B", "C")],
            }, vec![vec!["A", "B", "C"]]),
            ("fan-out A->{B,C}->D", WorkflowGraph {
                nodes: vec![trigger("A"), trigger("B"), trigger("C"), trigger("D")],
                edges: vec![edge("A", "B"), edge("A", "C"), edge("B", "D"), edge("C", "D")],
            }, vec![vec!["A", "B", "C", "D"], vec!["A", "C", "B", "D"]]),
            ("isolated node", WorkflowGraph {
                nodes: vec![trigger("X")],
                edges: vec![],
            }, vec![vec!["X"]]),
            ("cycle drops unreachable", WorkflowGraph {
                nodes: vec![trigger("A"), trigger("B"), trigger("C")],
                edges: vec![edge("A", "B"), edge("B", "C"), edge("C", "B")],
            }, vec![vec!["A"]]),
        ];
        for (label, graph, valid_orders) in &cases {
            let result = topo_sort(graph);
            assert!(valid_orders.iter().any(|v| v.iter().map(|s| s.to_string()).collect::<Vec<_>>() == result),
                "[{label}] got {result:?}, expected one of {valid_orders:?}");
        }
    }

    #[test]
    fn merge_args_input_does_not_overwrite_params() {
        let cases: Vec<(&str, JsonValue, JsonValue, JsonValue)> = vec![
            ("input fills gaps", json!({"entity": "deals"}), json!({"limit": 10}), json!({"entity": "deals", "limit": 10})),
            ("params take precedence", json!({"entity": "deals"}), json!({"entity": "WRONG"}), json!({"entity": "deals"})),
            ("non-object input ignored", json!({"x": 1}), json!("string"), json!({"x": 1})),
            ("non-object params returned as-is", json!(42), json!({"a": 1}), json!(42)),
        ];
        for (label, params, input, expected) in &cases {
            let result = merge_args(params, input);
            assert_eq!(&result, expected, "[{label}]");
        }
    }

    #[test]
    fn collect_inputs_merges_upstream_outputs() {
        let graph = WorkflowGraph {
            nodes: vec![trigger("A"), trigger("B"), trigger("C")],
            edges: vec![edge("A", "C"), edge("B", "C")],
        };
        let mut outputs = HashMap::new();
        outputs.insert("A".into(), json!({"x": 1}));
        outputs.insert("B".into(), json!({"y": 2, "x": 99}));

        let result = collect_inputs("C", &graph, &outputs);
        assert_eq!(result["x"], 1, "first edge wins on conflict");
        assert_eq!(result["y"], 2);
    }

    #[test]
    fn collect_inputs_strips_branch_metadata() {
        let graph = WorkflowGraph {
            nodes: vec![trigger("A"), trigger("B")],
            edges: vec![edge("A", "B")],
        };
        let mut outputs = HashMap::new();
        outputs.insert("A".into(), json!({"x": 1, "_branch": 0}));

        let result = collect_inputs("B", &graph, &outputs);
        assert_eq!(result, json!({"x": 1}));
    }

    #[test]
    fn if_node_branches_on_truthiness() {
        let cases: Vec<(&str, JsonValue, u8)> = vec![
            ("true", json!(true), 0),
            ("false", json!(false), 1),
            ("null", json!(null), 1),
            ("non-empty string", json!("yes"), 0),
            ("empty string", json!(""), 1),
            ("zero", json!(0), 1),
            ("non-zero", json!(42), 0),
            ("empty array", json!([]), 1),
            ("non-empty array", json!([1]), 0),
        ];
        for (label, condition, expected_branch) in &cases {
            let params = json!({"condition": condition});
            let (status, output, _) = execute_if(&params, &json!({}));
            assert_eq!(status, WorkflowNodeRunStatus::Succeeded);
            assert_eq!(output["_branch"], json!(*expected_branch), "[{label}]");
        }
    }

    #[test]
    fn switch_node_matches_case() {
        let params = json!({"value": "b", "cases": ["a", "b", "c"]});
        let (_, output, _) = execute_switch(&params, &json!({}));
        assert_eq!(output["_branch"], json!(1));

        let params = json!({"value": "z", "cases": ["a", "b"]});
        let (_, output, _) = execute_switch(&params, &json!({}));
        assert_eq!(output["_branch"], json!(2), "default = cases.len()");
    }

    #[test]
    fn should_execute_respects_branch_routing() {
        let graph = WorkflowGraph {
            nodes: vec![trigger("if"), trigger("yes"), trigger("no")],
            edges: vec![edge_branch("if", "yes", 0), edge_branch("if", "no", 1)],
        };
        let mut outputs = HashMap::new();
        outputs.insert("if".into(), json!({"_branch": 0}));
        let mut branches = HashMap::new();
        branches.insert("if".into(), 0u8);

        assert!(should_execute("yes", &graph, &outputs, &branches));
        assert!(!should_execute("no", &graph, &outputs, &branches));
    }
}
