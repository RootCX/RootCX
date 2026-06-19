use serde_json::{Value as JsonValue, json};
use rootcx_types::Item;

/// Encode node output (items per port) into the canonical JSON format.
pub(crate) fn encode(output: &[Vec<Item>]) -> JsonValue {
    json!(output.iter().map(|port|
        port.iter().map(|item| json!({ "json": item.json })).collect::<Vec<_>>()
    ).collect::<Vec<_>>())
}

/// Decode items from a specific output port. Handles legacy single-value format.
pub(crate) fn decode_port(output: &JsonValue, port: u8) -> Vec<Item> {
    if let Some(ports) = output.as_array() {
        if let Some(port_data) = ports.get(port as usize) {
            if let Some(items_arr) = port_data.as_array() {
                return items_arr.iter().map(|v| {
                    Item { json: v.get("json").cloned().unwrap_or_else(|| v.clone()) }
                }).collect();
            }
        }
        return vec![];
    }
    if output.is_null() { return vec![]; }
    vec![Item { json: output.clone() }]
}

/// Which output ports carry at least one item.
pub(crate) fn active_ports(output: &JsonValue) -> Vec<u8> {
    output.as_array().map(|ports| ports.iter().enumerate()
        .filter(|(_, p)| p.as_array().map(|a| !a.is_empty()).unwrap_or(false))
        .map(|(i, _)| i as u8).collect()
    ).unwrap_or_default()
}

/// Extract item[index] from port, returning its `json` field (or Null).
pub(crate) fn node_item_json(output: &JsonValue, port: u8, index: usize) -> JsonValue {
    output.as_array()
        .and_then(|ports| ports.get(port as usize))
        .and_then(|port| port.as_array())
        .and_then(|items| items.get(index))
        .and_then(|item| item.get("json").cloned())
        .unwrap_or(JsonValue::Null)
}

/// All items' json values from a port as an array.
pub(crate) fn all_items_json(output: &JsonValue, port: u8) -> JsonValue {
    output.as_array()
        .and_then(|ports| ports.get(port as usize))
        .and_then(|port| port.as_array())
        .map(|items| {
            let jsons: Vec<JsonValue> = items.iter()
                .filter_map(|item| item.get("json").cloned())
                .collect();
            JsonValue::Array(jsons)
        })
        .unwrap_or_else(|| json!([]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn encode_roundtrip() {
        let output = vec![
            vec![Item { json: json!({"id": 1}) }, Item { json: json!({"id": 2}) }],
            vec![Item { json: json!({"id": 3}) }],
        ];
        let encoded = encode(&output);
        assert_eq!(encoded, json!([[{"json": {"id": 1}}, {"json": {"id": 2}}], [{"json": {"id": 3}}]]));
    }

    #[test]
    fn decode_port_new_format() {
        let output = json!([[{"json": {"id": 1}}, {"json": {"id": 2}}], [{"json": {"id": 3}}]]);
        let port0 = decode_port(&output, 0);
        assert_eq!(port0.len(), 2);
        assert_eq!(port0[0].json, json!({"id": 1}));
        let port1 = decode_port(&output, 1);
        assert_eq!(port1.len(), 1);
    }

    #[test]
    fn decode_port_legacy_format() {
        let output = json!({"name": "Acme", "count": 5});
        let items = decode_port(&output, 0);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].json, json!({"name": "Acme", "count": 5}));
    }

    #[test]
    fn active_ports_flags_nonempty() {
        let cases: Vec<(&str, JsonValue, Vec<u8>)> = vec![
            ("port 1 only", json!([[], [{"json": {}}]]), vec![1]),
            ("both ports", json!([[{"json": {}}], [{"json": {}}]]), vec![0, 1]),
            ("none", json!([[], []]), vec![]),
            ("not an array", json!({}), vec![]),
        ];
        for (label, output, expected) in cases {
            assert_eq!(active_ports(&output), expected, "[{label}]");
        }
    }

    #[test]
    fn node_item_json_extracts_correctly() {
        let output = json!([[{"json": {"a": 1}}, {"json": {"a": 2}}], [{"json": {"b": 3}}]]);
        assert_eq!(node_item_json(&output, 0, 0), json!({"a": 1}));
        assert_eq!(node_item_json(&output, 0, 1), json!({"a": 2}));
        assert_eq!(node_item_json(&output, 1, 0), json!({"b": 3}));
        assert_eq!(node_item_json(&output, 2, 0), JsonValue::Null);
        assert_eq!(node_item_json(&output, 0, 5), JsonValue::Null);
    }

    #[test]
    fn all_items_json_collects_port() {
        let output = json!([[{"json": {"a": 1}}, {"json": {"a": 2}}]]);
        assert_eq!(all_items_json(&output, 0), json!([{"a": 1}, {"a": 2}]));
        assert_eq!(all_items_json(&output, 1), json!([]));
    }
}
