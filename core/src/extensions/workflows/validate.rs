use std::collections::HashSet;

use rootcx_types::{WorkflowGraph, WorkflowNodeKind};
use serde_json::Value as JsonValue;

/// Validate graph structure before execution; returns all issues found.
pub(crate) fn validate(graph: &WorkflowGraph) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    let node_ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let id_set: HashSet<&str> = node_ids.iter().copied().collect();

    // 4. Duplicate node ids
    if id_set.len() != node_ids.len() {
        let mut seen = HashSet::new();
        for id in &node_ids {
            if !seen.insert(*id) {
                errors.push(format!("duplicate node id: {id}"));
            }
        }
    }

    // 3. No trigger
    let has_trigger = graph.nodes.iter().any(|n| matches!(n.kind, WorkflowNodeKind::Trigger { .. }));
    if !has_trigger {
        errors.push("graph has no trigger node".into());
    }

    // 1. Dangling edges
    for edge in &graph.edges {
        if !id_set.contains(edge.from.as_str()) {
            errors.push(format!("edge references nonexistent source node: {}", edge.from));
        }
        if !id_set.contains(edge.to.as_str()) {
            errors.push(format!("edge references nonexistent target node: {}", edge.to));
        }
    }

    // 2. Expression bindings to nonexistent nodes
    for node in &graph.nodes {
        collect_node_refs(&node.params, &id_set, &mut errors);
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

fn collect_node_refs(value: &JsonValue, valid_ids: &HashSet<&str>, errors: &mut Vec<String>) {
    match value {
        JsonValue::String(s) => extract_node_refs(s, valid_ids, errors),
        JsonValue::Object(map) => {
            for v in map.values() {
                collect_node_refs(v, valid_ids, errors);
            }
        }
        JsonValue::Array(arr) => {
            for v in arr {
                collect_node_refs(v, valid_ids, errors);
            }
        }
        _ => {}
    }
}

fn extract_node_refs(s: &str, valid_ids: &HashSet<&str>, errors: &mut Vec<String>) {
    let pattern = "$node[\"";
    let alt_pattern = "$node['";
    for pat in [pattern, alt_pattern] {
        let mut rest = s;
        while let Some(start) = rest.find(pat) {
            let after = &rest[start + pat.len()..];
            let quote = if pat == pattern { '"' } else { '\'' };
            if let Some(end) = after.find(quote) {
                let node_id = &after[..end];
                if !valid_ids.contains(node_id) {
                    let msg = format!("expression references nonexistent node: {node_id}");
                    if !errors.contains(&msg) {
                        errors.push(msg);
                    }
                }
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rootcx_types::{WorkflowEdge, WorkflowGraph, WorkflowNode, WorkflowNodeKind, TriggerKind};
    use serde_json::json;

    fn trigger_node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            kind: WorkflowNodeKind::Trigger { trigger: TriggerKind::Manual },
            label: None,
            params: json!({}),
            position: [0.0, 0.0],
        }
    }

    fn tool_node(id: &str, params: JsonValue) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            kind: WorkflowNodeKind::Tool { tool_name: "test_tool".into() },
            label: None,
            params,
            position: [0.0, 0.0],
        }
    }

    fn edge(from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge { from: from.into(), to: to.into(), from_output: 0, to_input: 0 }
    }

    #[test]
    fn valid_graph() {
        let graph = WorkflowGraph {
            nodes: vec![
                trigger_node("trigger_1"),
                tool_node("fetch", json!({"url": "{{ $node[\"trigger_1\"].json.url }}"})),
            ],
            edges: vec![edge("trigger_1", "fetch")],
        };
        assert!(validate(&graph).is_ok());
    }

    #[test]
    fn dangling_edge() {
        let graph = WorkflowGraph {
            nodes: vec![trigger_node("trigger_1")],
            edges: vec![edge("trigger_1", "nonexistent")],
        };
        let errs = validate(&graph).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("nonexistent")));
    }

    #[test]
    fn expression_references_nonexistent_node() {
        let graph = WorkflowGraph {
            nodes: vec![
                trigger_node("trigger_1"),
                tool_node("step", json!({"val": "{{ $node[\"ghost\"].json.email }}"})),
            ],
            edges: vec![edge("trigger_1", "step")],
        };
        let errs = validate(&graph).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("ghost")));
    }

    #[test]
    fn no_trigger() {
        let graph = WorkflowGraph {
            nodes: vec![tool_node("a", json!({}))],
            edges: vec![],
        };
        let errs = validate(&graph).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("no trigger")));
    }

    #[test]
    fn duplicate_node_ids() {
        let graph = WorkflowGraph {
            nodes: vec![trigger_node("dup"), trigger_node("dup")],
            edges: vec![],
        };
        let errs = validate(&graph).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("duplicate node id: dup")));
    }

    #[test]
    fn multiple_issues() {
        let graph = WorkflowGraph {
            nodes: vec![
                tool_node("a", json!({"x": "{{ $node[\"phantom\"].json.y }}"})),
                tool_node("a", json!({})),
            ],
            edges: vec![edge("a", "missing")],
        };
        let errs = validate(&graph).unwrap_err();
        assert!(errs.len() >= 3, "expected at least 3 errors, got: {errs:?}");
        assert!(errs.iter().any(|e| e.contains("duplicate")));
        assert!(errs.iter().any(|e| e.contains("no trigger")));
        assert!(errs.iter().any(|e| e.contains("missing") || e.contains("phantom")));
    }
}
