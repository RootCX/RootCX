use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::tools::{DispatchError, ToolContext, ToolRegistry};
use rootcx_types::{
    ControlKind, Item, WorkflowGraph, WorkflowNodeKind, WorkflowNodeRunStatus,
    WorkflowExecutionStatus,
};

use super::expr;

pub struct NodeRunResult {
    pub node_id: String,
    pub status: WorkflowNodeRunStatus,
    pub output: JsonValue,
    pub error: Option<String>,
}

/// DB-first executor. Each node's output is persisted immediately after execution.
/// A local cache (`run_outputs`) avoids re-querying PG for expression resolution
/// and input collection — the DB remains the source of truth for durability/debug.
pub async fn execute_dag(
    registry: &Arc<ToolRegistry>,
    pool: &PgPool,
    app_id: &str,
    user_id: Uuid,
    perms: &[String],
    graph: &WorkflowGraph,
    exec_id: Uuid,
) -> Vec<NodeRunResult> {
    let node_map: HashMap<&str, &rootcx_types::WorkflowNode> =
        graph.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let order = topo_sort(graph);
    let mut results = Vec::new();
    let mut active_ports: HashMap<String, Vec<u8>> = HashMap::new();
    let mut run_outputs: HashMap<String, JsonValue> = HashMap::new();

    for node_id in &order {
        let Some(node) = node_map.get(node_id.as_str()) else { continue };

        if !should_execute(node_id, graph, &active_ports) {
            continue;
        }

        let input_items = collect_inputs(node_id, graph, &run_outputs);
        let input_json = items_to_json(&input_items);
        let resolved_params = expr::resolve(&node.params, &input_json, &run_outputs);

        let (status, output_items, error) = match &node.kind {
            WorkflowNodeKind::Trigger { .. } => {
                (WorkflowNodeRunStatus::Succeeded, vec![vec![Item { json: json!({}) }]], None)
            }
            WorkflowNodeKind::Tool { tool_name } => {
                execute_tool_node(registry, pool, app_id, user_id, perms, tool_name, &resolved_params, &input_items).await
            }
            WorkflowNodeKind::Control { control } => {
                execute_control_node(control, &node.params, &input_items, &run_outputs)
            }
            _ => (WorkflowNodeRunStatus::Skipped, vec![vec![]], Some("not implemented".into())),
        };

        let ports_with_items: Vec<u8> = output_items.iter().enumerate()
            .filter(|(_, port)| !port.is_empty())
            .map(|(i, _)| i as u8)
            .collect();

        let output_json = items_output_to_json(&output_items);

        // Persist to DB (source of truth for debug/history).
        sqlx::query(
            "INSERT INTO rootcx_system.workflow_node_runs (execution_id, node_id, status, input, output, error, started_at, finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, now(), now())",
        ).bind(exec_id).bind(node_id).bind(status.as_str())
        .bind(&input_json).bind(&output_json).bind(&error)
        .execute(pool).await.ok();

        if status == WorkflowNodeRunStatus::Succeeded {
            active_ports.insert(node_id.clone(), ports_with_items);
            run_outputs.insert(node_id.clone(), output_json.clone());
        }
        results.push(NodeRunResult { node_id: node_id.clone(), status, output: output_json, error });

        if status == WorkflowNodeRunStatus::Failed { break; }
    }

    results
}

/// Full lifecycle: create execution, run DAG, finalize status.
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

    let mut graph: WorkflowGraph = serde_json::from_value(graph_json)
        .map_err(|e| format!("invalid graph: {e}"))?;

    // Inject trigger data into the trigger node's params if provided.
    if let Some(td) = &trigger_data {
        if let Some(trigger_node) = graph.nodes.iter_mut().find(|n| matches!(n.kind, WorkflowNodeKind::Trigger { .. })) {
            trigger_node.params = td.clone();
        }
    }

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

// ── Input collection ─────────────────────────────────────────────────

/// Assemble input items from the local run_outputs cache (no DB queries needed).
fn collect_inputs(node_id: &str, graph: &WorkflowGraph, run_outputs: &HashMap<String, JsonValue>) -> Vec<Item> {
    let mut items = Vec::new();
    for edge in graph.edges.iter().filter(|e| e.to == node_id) {
        if let Some(output_val) = run_outputs.get(&edge.from) {
            items.extend(extract_port_items(output_val, edge.from_output));
        }
    }
    items
}

/// Extract items from a specific output port of the stored output JSON.
/// Output format: `[[{json: ...}, ...], [{json: ...}, ...]]` (items per port)
/// Falls back to legacy single-value format for backwards compat.
fn extract_port_items(output: &JsonValue, port: u8) -> Vec<Item> {
    // New format: array of arrays (per-port items)
    if let Some(ports) = output.as_array() {
        if let Some(port_data) = ports.get(port as usize) {
            if let Some(items_arr) = port_data.as_array() {
                return items_arr.iter().map(|v| {
                    if let Some(json_field) = v.get("json") {
                        Item { json: json_field.clone() }
                    } else {
                        Item { json: v.clone() }
                    }
                }).collect();
            }
        }
        return vec![];
    }
    // Legacy format: a single JSON value (old executor output)
    if output.is_null() { return vec![]; }
    vec![Item { json: output.clone() }]
}

// ── Conversion helpers ──────────────────────────────────────────────

fn items_to_json(items: &[Item]) -> JsonValue {
    if items.is_empty() { return json!({}); }
    if items.len() == 1 { return items[0].json.clone(); }
    json!(items.iter().map(|i| &i.json).collect::<Vec<_>>())
}

fn items_output_to_json(output: &[Vec<Item>]) -> JsonValue {
    json!(output.iter().map(|port|
        port.iter().map(|item| json!({ "json": item.json })).collect::<Vec<_>>()
    ).collect::<Vec<_>>())
}

// ── Routing ─────────────────────────────────────────────────────────

fn should_execute(node_id: &str, graph: &WorkflowGraph, active_ports: &HashMap<String, Vec<u8>>) -> bool {
    let node = graph.nodes.iter().find(|n| n.id == node_id);
    let is_trigger = node.map(|n| matches!(n.kind, WorkflowNodeKind::Trigger { .. })).unwrap_or(false);

    let inbound: Vec<_> = graph.edges.iter().filter(|e| e.to == node_id).collect();
    if inbound.is_empty() { return is_trigger; }

    inbound.iter().any(|edge| {
        active_ports.get(&edge.from)
            .map(|ports| ports.contains(&edge.from_output))
            .unwrap_or(false)
    })
}

// ── Control nodes ───────────────────────────────────────────────────

fn execute_control_node(
    control: &ControlKind,
    raw_params: &JsonValue,
    input_items: &[Item],
    outputs: &HashMap<String, JsonValue>,
) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    match control {
        ControlKind::If => execute_if(raw_params, input_items, outputs),
        ControlKind::Switch => execute_switch(raw_params, input_items, outputs),
        ControlKind::Set => execute_set(raw_params, input_items, outputs),
        ControlKind::Merge => {
            (WorkflowNodeRunStatus::Succeeded, vec![input_items.to_vec()], None)
        }
        ControlKind::Stop => {
            let resolved = expr::resolve(raw_params, &json!({}), outputs);
            let msg = resolved.get("message").and_then(|v| v.as_str()).unwrap_or("workflow stopped");
            (WorkflowNodeRunStatus::Failed, vec![vec![]], Some(msg.into()))
        }
        ControlKind::Loop | ControlKind::Wait => {
            (WorkflowNodeRunStatus::Skipped, vec![vec![]], Some("not implemented".into()))
        }
    }
}

fn execute_if(raw_params: &JsonValue, items: &[Item], outputs: &HashMap<String, JsonValue>) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    let mut true_items = Vec::new();
    let mut false_items = Vec::new();
    for item in items {
        let resolved = expr::resolve(raw_params, &item.json, outputs);
        let condition = resolved.get("condition").unwrap_or(&JsonValue::Null);
        if is_truthy(condition) {
            true_items.push(item.clone());
        } else {
            false_items.push(item.clone());
        }
    }
    (WorkflowNodeRunStatus::Succeeded, vec![true_items, false_items], None)
}

fn execute_switch(raw_params: &JsonValue, items: &[Item], outputs: &HashMap<String, JsonValue>) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    let cases = raw_params.get("cases").and_then(|c| c.as_array());
    let num_cases = cases.map(|c| c.len()).unwrap_or(0);
    let mut ports: Vec<Vec<Item>> = (0..=num_cases).map(|_| Vec::new()).collect();

    for item in items {
        let resolved = expr::resolve(raw_params, &item.json, outputs);
        let value = resolved.get("value").unwrap_or(&JsonValue::Null);
        let idx = match cases {
            Some(arr) => arr.iter().position(|c| c == value).unwrap_or(num_cases),
            None => 0,
        };
        ports[idx].push(item.clone());
    }

    (WorkflowNodeRunStatus::Succeeded, ports, None)
}

fn execute_set(raw_params: &JsonValue, items: &[Item], outputs: &HashMap<String, JsonValue>) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    let output: Vec<Item> = if items.is_empty() {
        let resolved = expr::resolve(raw_params, &json!({}), outputs);
        let fields = resolved.get("fields").cloned().unwrap_or(json!({}));
        vec![Item { json: fields }]
    } else {
        items.iter().map(|item| {
            let resolved = expr::resolve(raw_params, &item.json, outputs);
            let fields = resolved.get("fields").cloned().unwrap_or(json!({}));
            let mut merged = item.json.clone();
            if let (Some(base), Some(patch)) = (merged.as_object_mut(), fields.as_object()) {
                for (k, v) in patch { base.insert(k.clone(), v.clone()); }
            }
            Item { json: merged }
        }).collect()
    };
    (WorkflowNodeRunStatus::Succeeded, vec![output], None)
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

// ── Tool node ───────────────────────────────────────────────────────

async fn execute_tool_node(
    registry: &Arc<ToolRegistry>,
    pool: &PgPool,
    app_id: &str,
    user_id: Uuid,
    perms: &[String],
    tool_name: &str,
    params: &JsonValue,
    input_items: &[Item],
) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    let tool = match registry.get(tool_name) {
        Some(t) => t,
        None => return (WorkflowNodeRunStatus::Failed, vec![vec![]], Some(format!("unknown tool: {tool_name}"))),
    };

    // If no input items, execute once with just params.
    let items_to_process = if input_items.is_empty() {
        vec![Item { json: json!({}) }]
    } else {
        input_items.to_vec()
    };

    let mut output_items = Vec::new();
    for item in &items_to_process {
        let args = merge_args(params, &item.json);
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

        let outcome = crate::tools::dispatch(tool_name, tool.clone(), &ctx).await;
        match outcome.value {
            Ok(v) => {
                // Normalize: if the tool returns an array, each element becomes an item.
                let new_items = normalize_tool_output(v);
                output_items.extend(new_items);
            }
            Err(DispatchError::PermissionDenied(e)) => {
                return (WorkflowNodeRunStatus::Failed, vec![vec![]], Some(e));
            }
            Err(DispatchError::ExecutionFailed(e)) => {
                return (WorkflowNodeRunStatus::Failed, vec![vec![]], Some(e));
            }
        }
    }

    (WorkflowNodeRunStatus::Succeeded, vec![output_items], None)
}

fn normalize_tool_output(value: JsonValue) -> Vec<Item> {
    if let Some(arr) = value.as_array() {
        return arr.iter().map(|v| Item { json: v.clone() }).collect();
    }
    // If the object has exactly one array-valued field, expand it as items.
    if let Some(obj) = value.as_object() {
        let arrays: Vec<_> = obj.values().filter_map(|v| v.as_array()).collect();
        if arrays.len() == 1 {
            return arrays[0].iter().map(|v| Item { json: v.clone() }).collect();
        }
    }
    vec![Item { json: value }]
}

// ── Helpers ─────────────────────────────────────────────────────────

pub(crate) fn merge_args(params: &JsonValue, input: &JsonValue) -> JsonValue {
    let mut base = params.clone();
    if let (Some(b), Some(i)) = (base.as_object_mut(), input.as_object()) {
        for (k, v) in i { b.entry(k.clone()).or_insert_with(|| v.clone()); }
    }
    base
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
        WorkflowNode { id: id.into(), kind: WorkflowNodeKind::Trigger { trigger: TriggerKind::Manual }, label: None, params: json!({}), position: [0.0, 0.0] }
    }
    fn control(id: &str, kind: ControlKind, params: JsonValue) -> WorkflowNode {
        WorkflowNode { id: id.into(), kind: WorkflowNodeKind::Control { control: kind }, label: None, params, position: [0.0, 0.0] }
    }
    fn edge(from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge { from: from.into(), to: to.into(), from_output: 0, to_input: 0 }
    }
    fn edge_branch(from: &str, to: &str, branch: u8) -> WorkflowEdge {
        WorkflowEdge { from: from.into(), to: to.into(), from_output: branch, to_input: 0 }
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
    fn if_node_partitions_items_per_item() {
        let items = vec![
            Item { json: json!({"name": "a", "active": true}) },
            Item { json: json!({"name": "b", "active": false}) },
            Item { json: json!({"name": "c", "active": true}) },
        ];
        // Condition uses expression: $json.active → resolved per item
        let params = json!({"condition": "{{ $json.active }}"});
        let outputs = HashMap::new();
        let (status, output, _) = execute_if(&params, &items, &outputs);
        assert_eq!(status, WorkflowNodeRunStatus::Succeeded);
        assert_eq!(output[0].len(), 2, "2 truthy items (active=true)");
        assert_eq!(output[1].len(), 1, "1 falsy item (active=false)");
    }

    #[test]
    fn if_node_static_condition() {
        let items = vec![Item { json: json!({"x": 1}) }, Item { json: json!({"x": 2}) }];
        let params = json!({"condition": true});
        let outputs = HashMap::new();
        let (_, output, _) = execute_if(&params, &items, &outputs);
        assert_eq!(output[0].len(), 2, "static true routes all to port 0");
        assert!(output[1].is_empty());
    }

    #[test]
    fn switch_partitions_items_across_ports() {
        let items = vec![Item { json: json!({"status": "b"}) }, Item { json: json!({"status": "a"}) }];
        let params = json!({"value": "{{ $json.status }}", "cases": ["a", "b", "c"]});
        let outputs = HashMap::new();
        let (_, output, _) = execute_switch(&params, &items, &outputs);
        assert_eq!(output[0].len(), 1, "status=a → port 0");
        assert_eq!(output[1].len(), 1, "status=b → port 1");
    }

    #[test]
    fn normalize_tool_output_expands_records() {
        let cases: Vec<(&str, JsonValue, usize)> = vec![
            ("records array", json!({"records": [{"id": 1}, {"id": 2}]}), 2),
            ("raw array", json!([{"a": 1}, {"a": 2}, {"a": 3}]), 3),
            ("single object", json!({"result": "ok"}), 1),
            ("null", json!(null), 1),
        ];
        for (label, value, expected_count) in cases {
            let items = normalize_tool_output(value);
            assert_eq!(items.len(), expected_count, "[{label}]");
        }
    }

    #[test]
    fn extract_port_items_new_format() {
        let output = json!([[{"json": {"id": 1}}, {"json": {"id": 2}}], [{"json": {"id": 3}}]]);
        let port0 = extract_port_items(&output, 0);
        assert_eq!(port0.len(), 2);
        assert_eq!(port0[0].json, json!({"id": 1}));
        let port1 = extract_port_items(&output, 1);
        assert_eq!(port1.len(), 1);
    }

    #[test]
    fn extract_port_items_legacy_format() {
        let output = json!({"name": "Acme", "count": 5});
        let items = extract_port_items(&output, 0);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].json, json!({"name": "Acme", "count": 5}));
    }

    #[test]
    fn should_execute_respects_branch_routing() {
        let graph = WorkflowGraph {
            nodes: vec![trigger("if"), trigger("yes"), trigger("no")],
            edges: vec![edge_branch("if", "yes", 0), edge_branch("if", "no", 1)],
        };
        // Only port 0 has items → only "yes" should execute
        let mut active = HashMap::new();
        active.insert("if".into(), vec![0u8]);
        assert!(should_execute("yes", &graph, &active));
        assert!(!should_execute("no", &graph, &active));

        // Both ports have items → both should execute
        active.insert("if".into(), vec![0, 1]);
        assert!(should_execute("yes", &graph, &active));
        assert!(should_execute("no", &graph, &active));
    }
}
