use std::collections::HashMap;

use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::routes::SharedRuntime;
use crate::tools::{DispatchError, ToolContext};
use rootcx_types::{WorkflowGraph, WorkflowNodeKind, WorkflowNodeRunStatus};

pub struct NodeRunResult {
    pub node_id: String,
    pub status: WorkflowNodeRunStatus,
    pub error: Option<String>,
}

pub async fn execute_dag(
    rt: &SharedRuntime,
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
    let mut results = Vec::new();

    for node_id in &order {
        let Some(node) = node_map.get(node_id.as_str()) else { continue };

        let input = collect_inputs(node_id, graph, &outputs);

        let (status, output, error) = match &node.kind {
            WorkflowNodeKind::Trigger { .. } => {
                (WorkflowNodeRunStatus::Succeeded, json!({}), None)
            }
            WorkflowNodeKind::Tool { tool_name } => {
                execute_tool_node(rt, pool, app_id, user_id, perms, tool_name, &node.params, &input).await
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
            outputs.insert(node_id.clone(), output);
        }
        results.push(NodeRunResult { node_id: node_id.clone(), status, error });

        if status == WorkflowNodeRunStatus::Failed { break; }
    }

    results
}

async fn execute_tool_node(
    rt: &SharedRuntime,
    pool: &sqlx::PgPool,
    app_id: &str,
    user_id: Uuid,
    perms: &[String],
    tool_name: &str,
    params: &JsonValue,
    input: &JsonValue,
) -> (WorkflowNodeRunStatus, JsonValue, Option<String>) {
    let tool = match rt.tool_registry().get(tool_name) {
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
                    for (k, v) in obj { merged.entry(k.clone()).or_insert_with(|| v.clone()); }
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
    use rootcx_types::{WorkflowGraph, WorkflowNode, WorkflowEdge, WorkflowNodeKind, TriggerKind};

    fn node(id: &str) -> WorkflowNode {
        WorkflowNode { id: id.into(), kind: WorkflowNodeKind::Trigger { trigger: TriggerKind::Manual }, params: json!({}), position: [0.0, 0.0] }
    }
    fn edge(from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge { from: from.into(), to: to.into(), from_output: 0 }
    }

    #[test]
    fn topo_sort_linear_and_branching() {
        let cases: Vec<(&str, WorkflowGraph, Vec<Vec<&str>>)> = vec![
            ("linear A->B->C", WorkflowGraph {
                nodes: vec![node("A"), node("B"), node("C")],
                edges: vec![edge("A", "B"), edge("B", "C")],
            }, vec![vec!["A", "B", "C"]]),
            ("fan-out A->{B,C}->D", WorkflowGraph {
                nodes: vec![node("A"), node("B"), node("C"), node("D")],
                edges: vec![edge("A", "B"), edge("A", "C"), edge("B", "D"), edge("C", "D")],
            }, vec![vec!["A", "B", "C", "D"], vec!["A", "C", "B", "D"]]),
            ("isolated node", WorkflowGraph {
                nodes: vec![node("X")],
                edges: vec![],
            }, vec![vec!["X"]]),
            ("cycle drops unreachable", WorkflowGraph {
                nodes: vec![node("A"), node("B"), node("C")],
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
            nodes: vec![node("A"), node("B"), node("C")],
            edges: vec![edge("A", "C"), edge("B", "C")],
        };
        let mut outputs = HashMap::new();
        outputs.insert("A".into(), json!({"x": 1}));
        outputs.insert("B".into(), json!({"y": 2, "x": 99}));

        let result = collect_inputs("C", &graph, &outputs);
        assert_eq!(result["x"], 1, "first edge wins on conflict");
        assert_eq!(result["y"], 2);
    }
}
