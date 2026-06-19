use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::tools::{BatchMode, DispatchError, ToolContext, ToolRegistry};
use rootcx_types::{ControlKind, Item, WorkflowGraph, WorkflowNodeKind, WorkflowNodeRunStatus};

use super::expr;

/// Outcome of one node, ready for the runner to persist and route. Pure: no DB
/// writes, no retry, no should-execute decision — the durable runner owns those.
/// Output ports are derived from `output_json` by the runner (single source).
pub(crate) struct NodeExecution {
    pub status: WorkflowNodeRunStatus,
    /// Items-per-port format: `[[{json:..}], ..]`.
    pub output_json: JsonValue,
    pub input_json: JsonValue,
    pub error: Option<String>,
    /// A Stop node: record it, then halt the whole execution (no further nodes).
    pub halt: bool,
}

/// Execute a single node against the current `run_outputs` cache. Input items are
/// collected per port from upstream outputs; expression params are resolved here.
pub(crate) async fn execute_node(
    registry: &Arc<ToolRegistry>,
    pool: &PgPool,
    app_id: &str,
    user_id: Uuid,
    perms: &[String],
    node: &rootcx_types::WorkflowNode,
    graph: &WorkflowGraph,
    run_outputs: &HashMap<String, JsonValue>,
    exec_id: Uuid,
) -> NodeExecution {
    let input_by_port = collect_inputs_by_port(&node.id, graph, run_outputs);
    let input_items: Vec<Item> = input_by_port.iter().flatten().cloned().collect();
    let input_json = items_to_json(&input_items);

    let (status, output_items, error) = match &node.kind {
        WorkflowNodeKind::Trigger { .. } => {
            // Emit params as the seed item (= trigger_data injected by create_execution).
            (WorkflowNodeRunStatus::Succeeded, vec![vec![Item { json: node.params.clone() }]], None)
        }
        WorkflowNodeKind::Tool { tool_name } => {
            let idem_base = format!("{exec_id}:{}", node.id);
            execute_tool_node(registry, pool, app_id, user_id, perms, tool_name, &node.params, &input_items, &idem_base, run_outputs).await
        }
        WorkflowNodeKind::Control { control } => {
            execute_control_node(control, &node.params, &input_items, &input_by_port, run_outputs)
        }
        _ => (WorkflowNodeRunStatus::Skipped, vec![vec![]], Some("not implemented".into())),
    };

    NodeExecution {
        status,
        output_json: items_output_to_json(&output_items),
        input_json,
        error,
        halt: matches!(&node.kind, WorkflowNodeKind::Control { control: ControlKind::Stop }),
    }
}

// ── Input collection ─────────────────────────────────────────────────

/// Assemble input items grouped by destination input port (`to_input`), from the
/// local run_outputs cache. Generic nodes flatten this; Merge keeps the grouping
/// to align/join branches. No DB queries needed.
fn collect_inputs_by_port(node_id: &str, graph: &WorkflowGraph, run_outputs: &HashMap<String, JsonValue>) -> Vec<Vec<Item>> {
    let inbound: Vec<_> = graph.edges.iter().filter(|e| e.to == node_id).collect();
    let Some(max_port) = inbound.iter().map(|e| e.to_input).max() else { return vec![] };
    let mut ports: Vec<Vec<Item>> = (0..=max_port).map(|_| Vec::new()).collect();
    for edge in inbound {
        if let Some(output_val) = run_outputs.get(&edge.from) {
            ports[edge.to_input as usize].extend(extract_port_items(output_val, edge.from_output));
        }
    }
    ports
}

fn extract_port_items(output: &JsonValue, port: u8) -> Vec<Item> {
    super::items::decode_port(output, port)
}

// ── Conversion helpers ──────────────────────────────────────────────

fn items_to_json(items: &[Item]) -> JsonValue {
    if items.is_empty() { return json!({}); }
    if items.len() == 1 { return items[0].json.clone(); }
    json!(items.iter().map(|i| &i.json).collect::<Vec<_>>())
}

fn items_output_to_json(output: &[Vec<Item>]) -> JsonValue {
    super::items::encode(output)
}

// ── Routing ─────────────────────────────────────────────────────────

pub(crate) fn should_execute(node_id: &str, graph: &WorkflowGraph, active_ports: &HashMap<String, Vec<u8>>) -> bool {
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
    input_by_port: &[Vec<Item>],
    outputs: &HashMap<String, JsonValue>,
) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    match control {
        ControlKind::If => execute_if(raw_params, input_items, outputs),
        ControlKind::Switch => execute_switch(raw_params, input_items, outputs),
        ControlKind::Set => execute_set(raw_params, input_items, outputs),
        ControlKind::Merge => execute_merge(raw_params, input_by_port, outputs),
        ControlKind::Stop => {
            // Graceful halt: the node succeeds and surfaces its reason as output;
            // the runner stops the whole execution here (see `NodeExecution.halt`).
            let resolved = expr::resolve(raw_params, &json!({}), outputs);
            let msg = resolved.get("message").and_then(|v| v.as_str()).unwrap_or("workflow stopped");
            (WorkflowNodeRunStatus::Succeeded, vec![vec![Item { json: json!({ "stopped": true, "message": msg }) }]], None)
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
            overlay(&mut merged, &fields);
            Item { json: merged }
        }).collect()
    };
    (WorkflowNodeRunStatus::Succeeded, vec![output], None)
}

/// Merge branches per n8n's three modes:
///   - `append` (default): concatenate every input port in order.
///   - `combineByPosition`: zip ports index-wise, overlaying later ports onto earlier.
///   - `combineByField`: inner-join port 0 and port 1 on `field1`/`field2` (`field2`
///     defaults to `field1`), overlaying matched right items onto left.
fn execute_merge(raw_params: &JsonValue, input_by_port: &[Vec<Item>], outputs: &HashMap<String, JsonValue>) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    let p = expr::resolve(raw_params, &json!({}), outputs);
    let out = match p.get("mode").and_then(|v| v.as_str()).unwrap_or("append") {
        "combineByPosition" => merge_by_position(input_by_port),
        "combineByField" => merge_by_field(input_by_port, &p),
        _ => input_by_port.iter().flatten().cloned().collect(),
    };
    (WorkflowNodeRunStatus::Succeeded, vec![out], None)
}

fn merge_by_position(ports: &[Vec<Item>]) -> Vec<Item> {
    let len = ports.iter().map(|p| p.len()).max().unwrap_or(0);
    (0..len).map(|i| {
        let mut acc = json!({});
        for port in ports {
            if let Some(item) = port.get(i) { overlay(&mut acc, &item.json); }
        }
        Item { json: acc }
    }).collect()
}

fn merge_by_field(ports: &[Vec<Item>], p: &JsonValue) -> Vec<Item> {
    let f1 = p.get("field1").or_else(|| p.get("field")).and_then(|v| v.as_str()).unwrap_or("id");
    let f2 = p.get("field2").and_then(|v| v.as_str()).unwrap_or(f1);
    let left = ports.first().map(Vec::as_slice).unwrap_or(&[]);
    let right = ports.get(1).map(Vec::as_slice).unwrap_or(&[]);
    let mut out = Vec::new();
    for l in left {
        let Some(key) = l.json.get(f1) else { continue };
        for r in right.iter().filter(|r| r.json.get(f2) == Some(key)) {
            let mut merged = l.json.clone();
            overlay(&mut merged, &r.json);
            out.push(Item { json: merged });
        }
    }
    out
}

/// Shallow-merge `patch`'s fields onto `base` (patch wins on clashes).
fn overlay(base: &mut JsonValue, patch: &JsonValue) {
    if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
        for (k, v) in p { b.insert(k.clone(), v.clone()); }
    }
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
    raw_params: &JsonValue,
    input_items: &[Item],
    idem_base: &str,
    outputs: &HashMap<String, JsonValue>,
) -> (WorkflowNodeRunStatus, Vec<Vec<Item>>, Option<String>) {
    let tool = match registry.get(tool_name) {
        Some(t) => t,
        None => return (WorkflowNodeRunStatus::Failed, vec![vec![]], Some(format!("unknown tool: {tool_name}"))),
    };

    // Node param overrides the tool's declared default (e.g. force a read per-item).
    let mode = raw_params.get("batchMode").and_then(|v| v.as_str())
        .and_then(BatchMode::parse)
        .unwrap_or_else(|| tool.batch_mode());
    let mut params_base = raw_params.clone();
    if let Some(o) = params_base.as_object_mut() { o.remove("batchMode"); }

    // Once: a single dispatch with node params, ignoring item fan-out.
    // PerItem: map over items (one dispatch each), the input filling param gaps.
    let items_to_process = match mode {
        BatchMode::Once => vec![Item { json: json!({}) }],
        BatchMode::PerItem if input_items.is_empty() => vec![Item { json: json!({}) }],
        BatchMode::PerItem => input_items.to_vec(),
    };

    let mut output_items = Vec::new();
    for (i, item) in items_to_process.iter().enumerate() {
        // Resolve per-item: $json = this item so {{ $json.field }} works in tool params.
        let resolved = expr::resolve(&params_base, &item.json, outputs);
        let args = merge_args(&resolved, &item.json);
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
            // Stable across attempts and resume: same (exec, node, item) → same key.
            idempotency_key: Some(format!("{idem_base}:{i}")),
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

    fn item(v: JsonValue) -> Item { Item { json: v } }

    #[test]
    fn merge_append_concatenates_all_ports_in_order() {
        // Absent mode defaults to append; both forms stack every port's items.
        let ports = vec![
            vec![item(json!({"id": 1})), item(json!({"id": 2}))],
            vec![item(json!({"id": 3}))],
        ];
        for params in [json!({"mode": "append"}), json!({})] {
            let (status, out, _) = execute_merge(&params, &ports, &HashMap::new());
            assert_eq!(status, WorkflowNodeRunStatus::Succeeded, "[{params}]");
            assert_eq!(out[0].len(), 3, "[{params}]");
            assert_eq!(out[0][2].json, json!({"id": 3}), "[{params}] order preserved");
        }
    }

    #[test]
    fn merge_by_position_zips_and_overlays_to_longest() {
        let ports = vec![
            vec![item(json!({"id": 1, "name": "a"})), item(json!({"id": 2}))],
            vec![item(json!({"email": "x@y.z"}))],
        ];
        let (_, out, _) = execute_merge(&json!({"mode": "combineByPosition"}), &ports, &HashMap::new());
        // length = longest port; row 0 merges both, the unpaired row 1 passes through.
        assert_eq!(out[0].len(), 2);
        assert_eq!(out[0][0].json, json!({"id": 1, "name": "a", "email": "x@y.z"}));
        assert_eq!(out[0][1].json, json!({"id": 2}));
    }

    #[test]
    fn merge_by_field_inner_joins_port0_and_port1() {
        let left = vec![item(json!({"uid": 1, "name": "a"})), item(json!({"uid": 2, "name": "b"}))];
        let right = vec![item(json!({"id": 1, "role": "admin"})), item(json!({"id": 9})), item(json!({"uid": 2, "role": "ops"}))];
        let ports = vec![left, right];
        // (params, expected_len, expected_first_or_none)
        let cases: Vec<(&str, JsonValue, usize, Option<JsonValue>)> = vec![
            ("explicit field1/field2",
                json!({"mode": "combineByField", "field1": "uid", "field2": "id"}),
                1, Some(json!({"uid": 1, "name": "a", "id": 1, "role": "admin"}))),
            ("field2 defaults to field1",
                json!({"mode": "combineByField", "field1": "uid"}),
                1, Some(json!({"uid": 2, "name": "b", "role": "ops"}))),
            ("no overlapping key → empty",
                json!({"mode": "combineByField", "field1": "name", "field2": "role"}),
                0, None),
        ];
        for (label, params, len, first) in cases {
            let (_, out, _) = execute_merge(&params, &ports, &HashMap::new());
            assert_eq!(out[0].len(), len, "[{label}]");
            if let Some(expected) = first { assert_eq!(out[0][0].json, expected, "[{label}]"); }
        }
    }

    #[test]
    fn batch_mode_parse_roundtrips() {
        assert_eq!(BatchMode::parse("once"), Some(BatchMode::Once));
        assert_eq!(BatchMode::parse("perItem"), Some(BatchMode::PerItem));
        assert_eq!(BatchMode::parse("bogus"), None);
    }

    #[test]
    fn collect_inputs_by_port_groups_by_to_input() {
        let mut outputs = HashMap::new();
        outputs.insert("a".to_string(), json!([[{"json": {"x": 1}}]]));
        outputs.insert("b".to_string(), json!([[{"json": {"y": 2}}]]));
        let graph = WorkflowGraph {
            nodes: vec![trigger("a"), trigger("b"), trigger("m")],
            edges: vec![
                WorkflowEdge { from: "a".into(), to: "m".into(), from_output: 0, to_input: 0 },
                WorkflowEdge { from: "b".into(), to: "m".into(), from_output: 0, to_input: 1 },
            ],
        };
        let ports = collect_inputs_by_port("m", &graph, &outputs);
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0][0].json, json!({"x": 1}));
        assert_eq!(ports[1][0].json, json!({"y": 2}));
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
