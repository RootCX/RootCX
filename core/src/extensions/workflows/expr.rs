use std::collections::HashMap;
use serde_json::{Value as JsonValue, json};

/// Resolve expression bindings in a JsonValue tree.
/// Strings containing `{{ expr }}` are evaluated against node outputs.
/// Supported expressions:
///   - `$node["id"].json.path.to.field`
///   - `$json.path` (shorthand for the current node's input)
///   - `$input.path` (alias for $json)
/// Non-string values and strings without `{{ }}` pass through unchanged.
pub fn resolve(value: &JsonValue, input: &JsonValue, outputs: &HashMap<String, JsonValue>) -> JsonValue {
    match value {
        JsonValue::String(s) => resolve_string(s, input, outputs),
        JsonValue::Object(map) => {
            let resolved: serde_json::Map<String, JsonValue> = map.iter()
                .map(|(k, v)| (k.clone(), resolve(v, input, outputs)))
                .collect();
            JsonValue::Object(resolved)
        }
        JsonValue::Array(arr) => {
            JsonValue::Array(arr.iter().map(|v| resolve(v, input, outputs)).collect())
        }
        other => other.clone(),
    }
}

fn resolve_string(s: &str, input: &JsonValue, outputs: &HashMap<String, JsonValue>) -> JsonValue {
    let trimmed = s.trim();
    if let Some(expr) = trimmed.strip_prefix("{{").and_then(|r| r.strip_suffix("}}")) {
        eval_expr(expr.trim(), input, outputs)
    } else if trimmed.contains("{{") && trimmed.contains("}}") {
        let result = interpolate(s, input, outputs);
        JsonValue::String(result)
    } else {
        JsonValue::String(s.to_string())
    }
}

fn interpolate(s: &str, input: &JsonValue, outputs: &HashMap<String, JsonValue>) -> String {
    let mut result = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let expr = after[..end].trim();
            let val = eval_expr(expr, input, outputs);
            match &val {
                JsonValue::String(s) => result.push_str(s),
                JsonValue::Null => result.push_str("null"),
                other => result.push_str(&other.to_string()),
            }
            rest = &after[end + 2..];
        } else {
            result.push_str("{{");
            rest = after;
        }
    }
    result.push_str(rest);
    result
}

fn eval_expr(expr: &str, input: &JsonValue, outputs: &HashMap<String, JsonValue>) -> JsonValue {
    if let Some(path) = expr.strip_prefix("$json.").or_else(|| expr.strip_prefix("$input.")) {
        navigate(input, path)
    } else if expr == "$json" || expr == "$input" {
        input.clone()
    } else if let Some(rest) = expr.strip_prefix("$node[") {
        parse_node_ref(rest, outputs)
    } else {
        JsonValue::Null
    }
}

fn parse_node_ref(rest: &str, outputs: &HashMap<String, JsonValue>) -> JsonValue {
    let quote = match rest.as_bytes().first() {
        Some(b'"') | Some(b'\'') => rest.as_bytes()[0] as char,
        _ => return JsonValue::Null,
    };
    let inner = &rest[1..];
    let end = match inner.find(quote) {
        Some(e) => e,
        None => return JsonValue::Null,
    };
    let (node_id, after) = (&inner[..end], &inner[end + 1..]);

    let after = after.strip_prefix(']').unwrap_or(after);

    let stored = match outputs.get(node_id) {
        Some(v) => v,
        None => return JsonValue::Null,
    };

    // Unwrap the items-per-port format: [[{json: ...}, ...], ...]
    // $node["X"].json → first item of port 0
    // $node["X"].all → all items of port 0 as array
    let first_item_json = stored.as_array()
        .and_then(|ports| ports.first())
        .and_then(|port| port.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.get("json"))
        .cloned()
        // Legacy fallback: stored value is the raw output itself
        .unwrap_or_else(|| stored.clone());

    if let Some(path) = after.strip_prefix(".all") {
        let path = path.strip_prefix('.').unwrap_or("");
        let all_items: JsonValue = stored.as_array()
            .and_then(|ports| ports.first())
            .and_then(|port| port.as_array())
            .map(|items| {
                let jsons: Vec<JsonValue> = items.iter()
                    .filter_map(|item| item.get("json").cloned())
                    .collect();
                JsonValue::Array(jsons)
            })
            .unwrap_or_else(|| json!([]));
        if path.is_empty() { all_items } else { navigate(&all_items, path) }
    } else {
        let path = after.strip_prefix(".json.").or_else(|| after.strip_prefix(".json"))
            .unwrap_or("");
        if path.is_empty() { first_item_json } else { navigate(&first_item_json, path) }
    }
}

fn navigate(value: &JsonValue, path: &str) -> JsonValue {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() { continue; }
        match current {
            JsonValue::Object(map) => {
                current = match map.get(segment) {
                    Some(v) => v,
                    None => return JsonValue::Null,
                };
            }
            JsonValue::Array(arr) => {
                if let Ok(idx) = segment.parse::<usize>() {
                    current = match arr.get(idx) {
                        Some(v) => v,
                        None => return JsonValue::Null,
                    };
                } else {
                    return JsonValue::Null;
                }
            }
            _ => return JsonValue::Null,
        }
    }
    current.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_expressions() {
        let mut outputs = HashMap::new();
        outputs.insert("fetch".into(), json!({"name": "Acme", "items": [1, 2, 3]}));
        let input = json!({"limit": 10, "nested": {"x": true}});

        let cases: Vec<(&str, JsonValue, JsonValue)> = vec![
            ("passthrough string", json!("hello"), json!("hello")),
            ("passthrough number", json!(42), json!(42)),
            ("$json.field", json!("{{ $json.limit }}"), json!(10)),
            ("$input.nested.x", json!("{{ $input.nested.x }}"), json!(true)),
            ("$node ref", json!("{{ $node[\"fetch\"].json.name }}"), json!("Acme")),
            ("$node array index", json!("{{ $node[\"fetch\"].json.items.1 }}"), json!(2)),
            ("$node whole output", json!("{{ $node[\"fetch\"].json }}"), json!({"name": "Acme", "items": [1, 2, 3]})),
            ("missing node", json!("{{ $node[\"nope\"].json.x }}"), json!(null)),
            ("missing field", json!("{{ $json.unknown }}"), json!(null)),
            ("interpolation", json!("Hello {{ $node[\"fetch\"].json.name }}!"), json!("Hello Acme!")),
            ("object recursion", json!({"key": "{{ $json.limit }}"}), json!({"key": 10})),
            ("array recursion", json!(["{{ $json.limit }}", "static"]), json!([10, "static"])),
            // sad paths
            ("empty expression", json!("{{  }}"), json!(null)),
            ("unknown variable", json!("{{ $foobar }}"), json!(null)),
            ("unclosed braces", json!("{{ $json.limit"), json!("{{ $json.limit")),
            ("bare $json (whole input)", json!("{{ $json }}"), json!({"limit": 10, "nested": {"x": true}})),
            ("single quotes in node ref", json!("{{ $node['fetch'].json.name }}"), json!("Acme")),
        ];
        for (label, value, expected) in &cases {
            let result = resolve(value, &input, &outputs);
            assert_eq!(&result, expected, "[{label}]");
        }
    }
}
