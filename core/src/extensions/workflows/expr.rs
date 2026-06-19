use std::collections::HashMap;
use serde_json::Value as JsonValue;

use boa_engine::{Context, JsValue, Source, js_string};
use boa_engine::property::Attribute;

/// Resolve expression bindings in a JsonValue tree.
/// Strings containing `{{ expr }}` are evaluated as JavaScript with access to:
///   - `$json` / `$input` — the current node's input
///   - `$node` — object keyed by node id, each with a `.json` (first item, port 0)
/// A single Boa Context is created per `resolve()` call and reused for every
/// expression in the tree. Eval failures return `null` and log a warning.
pub fn resolve(value: &JsonValue, input: &JsonValue, outputs: &HashMap<String, JsonValue>) -> JsonValue {
    let mut ctx = Context::default();

    // Inject $json and $input (same value, two aliases).
    let input_js = JsValue::from_json(input, &mut ctx).unwrap_or(JsValue::Null);
    ctx.register_global_property(js_string!("$json"), input_js.clone(), Attribute::all()).ok();
    ctx.register_global_property(js_string!("$input"), input_js, Attribute::all()).ok();

    // Build $node once: pre-extract first item per node, convert to JS.
    // Fallback for legacy (non-items-per-port) format: use the stored value directly.
    let node_json: serde_json::Map<String, JsonValue> = outputs.iter()
        .map(|(id, stored)| {
            let first = super::items::node_item_json(stored, 0, 0);
            let resolved = if first.is_null() && !stored.as_array().map(|a| a.first().and_then(|p| p.as_array()).is_some()).unwrap_or(false) {
                stored.clone()
            } else { first };
            (id.clone(), serde_json::json!({ "json": resolved }))
        })
        .collect();
    let node_js = JsValue::from_json(&JsonValue::Object(node_json), &mut ctx).unwrap_or(JsValue::Null);
    ctx.register_global_property(js_string!("$node"), node_js, Attribute::all()).ok();

    resolve_tree(value, &mut ctx)
}

fn resolve_tree(value: &JsonValue, ctx: &mut Context) -> JsonValue {
    match value {
        JsonValue::String(s) => resolve_string(s, ctx),
        JsonValue::Object(map) => {
            let resolved: serde_json::Map<String, JsonValue> = map.iter()
                .map(|(k, v)| (k.clone(), resolve_tree(v, ctx)))
                .collect();
            JsonValue::Object(resolved)
        }
        JsonValue::Array(arr) => {
            JsonValue::Array(arr.iter().map(|v| resolve_tree(v, ctx)).collect())
        }
        other => other.clone(),
    }
}

fn resolve_string(s: &str, ctx: &mut Context) -> JsonValue {
    let trimmed = s.trim();
    if let Some(expr) = trimmed.strip_prefix("{{").and_then(|r| r.strip_suffix("}}")) {
        eval_js(expr.trim(), ctx)
    } else if trimmed.contains("{{") && trimmed.contains("}}") {
        let result = interpolate(s, ctx);
        JsonValue::String(result)
    } else {
        JsonValue::String(s.to_string())
    }
}

fn interpolate(s: &str, ctx: &mut Context) -> String {
    let mut result = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let val = eval_js(after[..end].trim(), ctx);
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

fn eval_js(expr: &str, ctx: &mut Context) -> JsonValue {
    if expr.is_empty() { return JsonValue::Null; }
    match ctx.eval(Source::from_bytes(expr)) {
        Ok(val) => js_to_json(&val, ctx),
        Err(e) => {
            tracing::warn!("workflow expression eval failed: {e} | expr: {expr}");
            JsonValue::Null
        }
    }
}

/// Convert JsValue → serde_json::Value, preserving integer semantics (JS has only
/// f64, but workflow comparisons expect 10 == 10 not 10.0).
fn js_to_json(value: &JsValue, ctx: &mut Context) -> JsonValue {
    if value.is_undefined() { return JsonValue::Null; }
    match value.to_json(ctx) {
        Ok(v) => fixup_integers(v),
        Err(_) => JsonValue::Null,
    }
}

fn fixup_integers(v: JsonValue) -> JsonValue {
    match v {
        JsonValue::Number(n) => {
            if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                    return serde_json::json!(f as i64);
                }
            }
            JsonValue::Number(n)
        }
        JsonValue::Array(arr) => JsonValue::Array(arr.into_iter().map(fixup_integers).collect()),
        JsonValue::Object(map) => JsonValue::Object(map.into_iter().map(|(k, v)| (k, fixup_integers(v))).collect()),
        other => other,
    }
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
            ("$node array index", json!("{{ $node[\"fetch\"].json.items[1] }}"), json!(2)),
            ("$node whole output", json!("{{ $node[\"fetch\"].json }}"), json!({"name": "Acme", "items": [1, 2, 3]})),
            ("missing node", json!("{{ $node[\"nope\"] }}"), json!(null)),
            ("missing field", json!("{{ $json.unknown }}"), json!(null)),
            ("interpolation", json!("Hello {{ $node[\"fetch\"].json.name }}!"), json!("Hello Acme!")),
            ("object recursion", json!({"key": "{{ $json.limit }}"}), json!({"key": 10})),
            ("array recursion", json!(["{{ $json.limit }}", "static"]), json!([10, "static"])),
            // Sad: invalid JS → null (not a panic)
            ("syntax error", json!("{{ if( }}"), json!(null)),
            ("runtime error", json!("{{ $json.limit.toFixed.boom() }}"), json!(null)),
            ("empty expression", json!("{{  }}"), json!(null)),
            ("unclosed braces", json!("{{ $json.limit"), json!("{{ $json.limit")),
            ("bare $json", json!("{{ $json }}"), json!({"limit": 10, "nested": {"x": true}})),
            ("single quotes in node ref", json!("{{ $node['fetch'].json.name }}"), json!("Acme")),
        ];
        for (label, value, expected) in &cases {
            let result = resolve(value, &input, &outputs);
            assert_eq!(&result, expected, "[{label}]");
        }
    }

    #[test]
    fn node_ref_with_items_per_port_format() {
        let mut outputs = HashMap::new();
        outputs.insert("query".into(), json!([[{"json": {"email": "a@b.c", "id": 1}}, {"json": {"email": "x@y.z", "id": 2}}]]));
        let input = json!({});

        let cases: Vec<(&str, JsonValue, JsonValue)> = vec![
            ("first item field", json!("{{ $node[\"query\"].json.email }}"), json!("a@b.c")),
            ("first item whole", json!("{{ $node[\"query\"].json }}"), json!({"email": "a@b.c", "id": 1})),
            ("first item nested access", json!("{{ $node[\"query\"].json.id }}"), json!(1)),
        ];
        for (label, value, expected) in &cases {
            let result = resolve(value, &input, &outputs);
            assert_eq!(&result, expected, "[{label}]");
        }
    }

    #[test]
    fn if_condition_with_nullable_fields() {
        let outputs = HashMap::new();
        let with_email = json!({"id":"abc","city":"SF","email":"vic@test.com","phone":null,"first_name":"Vic","last_name":"T"});
        let without_email = json!({"id":"xyz","city":"Berlin","email":null,"phone":null,"first_name":"No","last_name":"E"});
        let params = json!({"condition": "{{ $json.email && $json.email.length > 0 }}"});

        let r1 = resolve(&params, &with_email, &outputs);
        assert_eq!(r1.get("condition"), Some(&json!(true)), "email present → true");

        let r2 = resolve(&params, &without_email, &outputs);
        assert_eq!(r2.get("condition"), Some(&json!(null)), "email null → null (falsy)");
    }

    #[test]
    fn js_operators_and_expressions() {
        let outputs = HashMap::new();
        let input = json!({"age": 25, "name": "Alice", "active": true});

        let cases: Vec<(&str, JsonValue, JsonValue)> = vec![
            ("comparison", json!("{{ $json.age >= 18 }}"), json!(true)),
            ("arithmetic", json!("{{ $json.age * 2 }}"), json!(50)),
            ("ternary", json!("{{ $json.active ? 'yes' : 'no' }}"), json!("yes")),
            ("string concat", json!("{{ $json.name + ' Smith' }}"), json!("Alice Smith")),
            ("logical AND", json!("{{ $json.active && $json.age > 20 }}"), json!(true)),
            ("template literal", json!("{{ `Hello ${$json.name}` }}"), json!("Hello Alice")),
            ("null coalescing", json!("{{ $json.missing ?? 'default' }}"), json!("default")),
            ("array method", json!("{{ [1,2,3].filter(x => x > 1).length }}"), json!(2)),
        ];
        for (label, value, expected) in &cases {
            let result = resolve(value, &input, &outputs);
            assert_eq!(&result, expected, "[{label}]");
        }
    }
}


