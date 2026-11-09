//! The MCP tools Core generates from installed apps (ADR 0016): one per action
//! an assistant may run (`<app>__<action>`), one per entity it may read
//! (`<app>__<entity>__list`). Hosts that review tools one by one see each
//! operation with its own inputs and hints. Listing and calling share one
//! decision, [`generated`], so a tool is callable exactly when it is listed.

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use rootcx_types::{EntityContract, FieldContract};
use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::data_types::FieldType;
use crate::governance::action_invocation::{Declaration, dispatch};
use crate::governance::authority::has_permission;
use crate::mcp_server::{oauth_tool_meta, scopes};
use crate::routes::SharedRuntime;
use crate::tools::describe_app::may_read;
use crate::tools::list_actions::invocable;

const SEPARATOR: &str = "__";
/// The registry tool a list call runs, and the grant it needs.
const READ_TOOL: &str = "query_data";
const READ_TOOL_GRANT: &str = "tool:query_data";
/// Ends a read tool's name, after the entity: `<app>__<entity>__list` can only
/// meet an action whose own id holds the separator.
const LIST_SUFFIX: &str = "list";
const DEFAULT_LIMIT: u64 = 25;
const MAX_LIMIT: u64 = 100;
/// The inputs of a list tool that are not filters.
const PAGING: [&str; 4] = ["orderBy", "order", "limit", "offset"];

/// What a generated tool runs.
pub(crate) enum Operation {
    Action { app: String, action: String, declaration: Declaration },
    List { app: String, entity: String, filters: Vec<String> },
}

impl Operation {
    pub(crate) fn read_only(&self) -> bool {
        match self {
            Self::Action { declaration, .. } => declaration.read_only,
            Self::List { .. } => true,
        }
    }

    /// Runs the operation for the person, through the Modules every other entry uses.
    pub(crate) async fn run(self, runtime: &SharedRuntime, identity: &Identity, input: JsonObject) -> Result<JsonValue, String> {
        match self {
            Self::Action { app, action, declaration } => {
                let caller = crate::ipc::RpcCaller::person(identity.user_id, &identity.email);
                let dispatcher = runtime.worker_manager().action_caller();
                dispatch(dispatcher.as_ref(), &app, &action, Some(&declaration), JsonValue::Object(input), caller)
                    .await
                    .map_err(|error| error.to_string())
            }
            Self::List { app, entity, filters } => {
                let args = query(&entity, &filters, input)?;
                crate::tools::routes::execute_as_person(runtime, identity, READ_TOOL, app, args)
                    .await
                    .map_err(|error| error.to_string())
            }
        }
    }
}

/// The tools this person may call, by name.
pub(crate) async fn list(pool: &PgPool, permissions: &[String]) -> Result<Vec<Tool>, ApiError> {
    Ok(generated(pool, permissions).await?.into_values().map(|(tool, _)| tool).collect())
}

/// The operation behind a listed tool name; `None` for any name [`list`] does not offer.
pub(crate) async fn resolve(pool: &PgPool, permissions: &[String], name: &str) -> Result<Option<Operation>, ApiError> {
    Ok(generated(pool, permissions).await?.remove(name).map(|(_, operation)| operation))
}

/// Every tool the installed apps give this person, by name, in name order.
async fn generated(pool: &PgPool, permissions: &[String]) -> Result<BTreeMap<String, (Tool, Operation)>, ApiError> {
    let apps: Vec<(String, String, JsonValue, JsonValue)> = sqlx::query_as(
        "SELECT id, name, COALESCE(manifest->'actions', '[]'::jsonb), COALESCE(manifest->'dataContract', '[]'::jsonb)
           FROM rootcx_system.apps WHERE status = 'installed' ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    let candidates = apps.into_iter().flat_map(|(app, app_name, actions, contract)| {
        let entities: Vec<EntityContract> = serde_json::from_value(contract).unwrap_or_else(|error| {
            tracing::warn!(app, %error, "unreadable data contract: no list tools");
            Vec::new()
        });
        app_tools(&app, &app_name, &actions, &entities, permissions)
    });
    Ok(unique(candidates))
}

/// One app's tools for these permissions: the actions an assistant may run,
/// and a read of each entity the permissions reach. A read runs the registry's
/// `query_data`, so it also needs that tool's grant, as on every other entry.
/// Derived entities belong to their app's worker (ADR 0012) and get no tool.
fn app_tools(app: &str, app_name: &str, actions: &JsonValue, entities: &[EntityContract], permissions: &[String]) -> Vec<(String, Tool, Operation)> {
    let actions = invocable(actions, app, permissions, true).filter_map(|(id, declaration)| {
        let (name, tool) = action_tool(app, app_name, id, &declaration)?;
        Some((name, tool, Operation::Action { app: app.to_string(), action: id.to_string(), declaration }))
    });
    let reads_data = has_permission(permissions, READ_TOOL_GRANT);
    let reads = entities
        .iter()
        .filter(|entity| reads_data && !entity.is_derived() && may_read(permissions, app, &entity.entity_name))
        .filter_map(|entity| {
            let (name, tool, filters) = list_tool(app, app_name, entity)?;
            Some((name, tool, Operation::List { app: app.to_string(), entity: entity.entity_name.clone(), filters }))
        });
    actions.chain(reads).collect()
}

/// The tools by name. A name two operations would share is dropped, so a call is never ambiguous.
fn unique(candidates: impl IntoIterator<Item = (String, Tool, Operation)>) -> BTreeMap<String, (Tool, Operation)> {
    let mut by_name: BTreeMap<String, Option<(Tool, Operation)>> = BTreeMap::new();
    for (name, tool, operation) in candidates {
        by_name
            .entry(name)
            .and_modify(|slot| {
                if let Some((tool, _)) = slot.take() {
                    tracing::warn!(tool = %tool.name, "two operations share this tool name: neither is offered");
                }
            })
            .or_insert(Some((tool, operation)));
    }
    by_name.into_iter().filter_map(|(name, slot)| Some((name, slot?))).collect()
}

/// `None` when the ids cannot form an MCP tool name (letters, digits, `_`, `-`, `.`; 128 at most).
fn tool_name(app: &str, operation: &str) -> Option<String> {
    let name = format!("{app}{SEPARATOR}{operation}");
    let valid = name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    valid.then_some(name)
}

fn tool(name: &str, title: String, description: String, input: JsonObject, result: JsonValue, hints: ToolAnnotations, read_only: bool) -> Tool {
    let output = object(json!({ "type": "object", "properties": { "result": result }, "required": ["result"] }));
    Tool::new(name.to_string(), description, Arc::new(input))
        .with_title(title)
        .with_raw_output_schema(Arc::new(output))
        .with_annotations(hints)
        .with_meta(oauth_tool_meta(scopes(read_only)))
}

/// The action as its manifest declares it. Undeclared hints take the worst
/// case, as for any MCP tool; an approved action is never read-only.
fn action_tool(app: &str, app_name: &str, id: &str, declaration: &Declaration) -> Option<(String, Tool)> {
    let name = tool_name(app, id)?;
    let action = &declaration.definition;
    let text = |key: &str| action.get(key).and_then(JsonValue::as_str).map(str::trim).filter(|value| !value.is_empty());
    let label = text("name").unwrap_or(id);
    let title = format!("{label} ({app_name})");
    let description = format!("{app_name}: {}", text("description").unwrap_or(label));
    let input = match action.get("inputSchema") {
        Some(JsonValue::Object(schema)) => schema.clone(),
        _ => object(json!({ "type": "object" })),
    };
    let result = action.get("outputSchema").filter(|schema| schema.is_object()).cloned().unwrap_or_else(|| json!({}));
    let hint = |key: &str| action.get("annotations").and_then(|hints| hints.get(key)).and_then(JsonValue::as_bool);
    let read_only = declaration.read_only;
    let hints = ToolAnnotations::with_title(title.clone())
        .read_only(read_only)
        .destructive(!read_only && hint("destructiveHint").unwrap_or(true))
        .idempotent(hint("idempotentHint").unwrap_or(read_only))
        .open_world(hint("openWorldHint").unwrap_or(true));
    let tool = tool(&name, title, description, input, result, hints, read_only);
    Some((name, tool))
}

/// A read of one entity: exact-match filters on its filterable fields, a sort
/// on its readable ones, a page. Sensitive fields are neither filters nor sorts,
/// and the read path never returns them.
fn list_tool(app: &str, app_name: &str, entity: &EntityContract) -> Option<(String, Tool, Vec<String>)> {
    let name = tool_name(app, &format!("{}{SEPARATOR}{LIST_SUFFIX}", entity.entity_name))?;
    let id_field: FieldContract = serde_json::from_value(json!({ "name": "id", "type": "uuid" })).ok()?;
    let readable: Vec<(&FieldContract, FieldType)> = std::iter::once(&id_field)
        .chain(entity.fields.iter().filter(|field| !field.sensitive && field.name != "id"))
        .filter_map(|field| Some((field, FieldType::from_field(field).ok()?)))
        .collect();
    let mut properties = serde_json::Map::new();
    let mut filters = Vec::new();
    for (field, field_type) in &readable {
        // A field named like a paging input stays readable but cannot be a filter.
        if PAGING.contains(&field.name.as_str()) {
            continue;
        }
        if let Some(schema) = filter_schema(field, field_type) {
            properties.insert(field.name.clone(), schema);
            filters.push(field.name.clone());
        }
    }
    let mut sorts: Vec<&str> = readable.iter().map(|(field, _)| field.name.as_str()).collect();
    for system in ["created_at", "updated_at"] {
        if !sorts.contains(&system) {
            sorts.push(system);
        }
    }
    properties.insert("orderBy".into(), json!({ "type": "string", "enum": sorts, "description": "Sort field (default: created_at)" }));
    properties.insert("order".into(), json!({ "type": "string", "enum": ["asc", "desc"], "description": "Sort direction (default: desc)" }));
    properties.insert("limit".into(), json!({ "type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": format!("Records per page (default: {DEFAULT_LIMIT})") }));
    properties.insert("offset".into(), json!({ "type": "integer", "minimum": 0, "description": "Records to skip (default: 0)" }));
    let input = object(json!({ "type": "object", "properties": properties, "additionalProperties": false }));
    let entity_name = &entity.entity_name;
    let title = format!("List {entity_name} ({app_name})");
    let description = format!(
        "{app_name}: list the {entity_name} records you may read, newest first. Each filter matches its field exactly. Read only."
    );
    let result = json!({
        "type": "object",
        "properties": { "data": { "type": "array", "items": { "type": "object" } }, "total": { "type": "integer" } },
    });
    let hints = ToolAnnotations::with_title(title.clone()).read_only(true).destructive(false).idempotent(true).open_world(false);
    let tool = tool(&name, title, description, input, result, hints, true);
    Some((name, tool, filters))
}

/// The JSON Schema of an exact-match filter on the field, or `None` when the
/// field cannot be compared exactly (JSON, files, arrays, timestamps).
fn filter_schema(field: &FieldContract, field_type: &FieldType) -> Option<JsonValue> {
    let schema = match field_type {
        FieldType::Text => match &field.enum_values {
            Some(values) if !values.is_empty() => json!({ "type": "string", "enum": values }),
            _ => json!({ "type": "string" }),
        },
        FieldType::Number => json!({ "type": "number" }),
        FieldType::Decimal(_) => json!({ "type": "string", "description": "Decimal number as text, for example 12.50" }),
        FieldType::Boolean => json!({ "type": "boolean" }),
        FieldType::Date => json!({ "type": "string", "format": "date" }),
        FieldType::Uuid | FieldType::EntityLink => json!({ "type": "string", "format": "uuid" }),
        FieldType::Timestamp | FieldType::Json | FieldType::File | FieldType::TextArray | FieldType::NumberArray => return None,
    };
    Some(schema)
}

/// The `query_data` arguments for a list call. Only the tool's own inputs are
/// accepted, and a filter takes one value: no operator reaches the query.
fn query(entity: &str, filters: &[String], input: JsonObject) -> Result<JsonValue, String> {
    let mut conditions = serde_json::Map::new();
    let mut args = json!({ "entity": entity, "limit": DEFAULT_LIMIT });
    for (key, value) in input {
        match key.as_str() {
            "orderBy" | "order" | "offset" => args[key.as_str()] = value,
            "limit" => args["limit"] = json!(value.as_u64().ok_or("limit must be a positive integer")?.clamp(1, MAX_LIMIT)),
            field if filters.iter().any(|filter| filter == field) => {
                if value.is_object() || value.is_array() {
                    return Err(format!("filter '{field}' takes a single value"));
                }
                conditions.insert(key, value);
            }
            _ => return Err(format!("unknown input '{key}'")),
        }
    }
    args["where"] = JsonValue::Object(conditions);
    Ok(args)
}

fn object(value: JsonValue) -> JsonObject {
    match value {
        JsonValue::Object(map) => map,
        _ => JsonObject::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(action: JsonValue) -> Option<Tool> {
        let id = action["id"].as_str().unwrap_or("").to_string();
        action_tool("desk", "Front desk", &id, &Declaration::from_definition(action)).map(|(_, tool)| tool)
    }

    fn entity(contract: JsonValue) -> EntityContract {
        serde_json::from_value(contract).expect("entity contract")
    }

    fn hints(tool: &Tool) -> (Option<bool>, Option<bool>, Option<bool>, Option<bool>) {
        let hints = tool.annotations.clone().expect("hints");
        (hints.read_only_hint, hints.destructive_hint, hints.idempotent_hint, hints.open_world_hint)
    }

    fn security(tool: &Tool) -> Option<&JsonValue> {
        tool.meta.as_ref().and_then(|meta| meta.get("securitySchemes"))
    }

    #[test]
    fn an_action_tool_states_exactly_what_its_action_declares() {
        for (label, declared, expected, scopes) in [
            ("declared read",
                json!({"id": "lookup", "name": "Look up", "annotations": {"readOnlyHint": true, "openWorldHint": false}}),
                (true, false, true, false), vec!["mcp:read"]),
            ("declared write",
                json!({"id": "stamp", "name": "Stamp", "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}}),
                (false, false, true, false), vec!["mcp:read", "mcp:write"]),
            ("no hints: worst case", json!({"id": "go", "name": "Go"}), (false, true, false, true), vec!["mcp:read", "mcp:write"]),
            ("approved action that claims to read",
                json!({"id": "pay", "name": "Pay", "authority": {"data": {}}, "annotations": {"readOnlyHint": true, "destructiveHint": false}}),
                (false, false, false, true), vec!["mcp:read", "mcp:write"]),
        ] {
            let tool = action(declared).unwrap_or_else(|| panic!("{label}: no tool"));
            assert_eq!(hints(&tool), (Some(expected.0), Some(expected.1), Some(expected.2), Some(expected.3)), "{label}");
            assert_eq!(security(&tool), Some(&json!([{"type": "oauth2", "scopes": scopes}])), "{label}");
        }

        let lookup = action(json!({"id": "lookup", "name": "Look up", "description": " Finds a booking. ", "inputSchema": {"type": "object", "required": ["q"]}})).unwrap();
        assert_eq!(lookup.name, "desk__lookup");
        assert_eq!(lookup.description.as_deref(), Some("Front desk: Finds a booking."));
        assert_eq!(JsonValue::Object((*lookup.input_schema).clone()), json!({"type": "object", "required": ["q"]}));

        let bare = action(json!({"id": "go", "description": "", "inputSchema": "not a schema"})).unwrap();
        assert_eq!(bare.description.as_deref(), Some("Front desk: go"), "an empty description falls back to the name, then the id");
        assert_eq!(JsonValue::Object((*bare.input_schema).clone()), json!({"type": "object"}));
    }

    #[test]
    fn a_list_tool_filters_and_sorts_only_on_readable_fields() {
        let bookings = entity(json!({"entityName": "bookings", "fields": [
            {"name": "guest", "type": "text"},
            {"name": "status", "type": "text", "enum_values": ["open", "closed"]},
            {"name": "nights", "type": "number"},
            {"name": "total", "type": "decimal", "precision": 10, "scale": 2},
            {"name": "paid", "type": "boolean"},
            {"name": "arrival", "type": "date"},
            {"name": "room_id", "type": "entity_link", "references": {"entity": "rooms", "field": "id"}},
            {"name": "notes", "type": "json"},
            {"name": "tags", "type": "[text]"},
            {"name": "passport", "type": "text", "sensitive": true},
            {"name": "order", "type": "text"}
        ]}));
        let (name, tool, filters) = list_tool("desk", "Front desk", &bookings).expect("list tool");
        assert_eq!(name, "desk__bookings__list");
        assert_eq!(filters, ["id", "guest", "status", "nights", "total", "paid", "arrival", "room_id"],
            "JSON, arrays, sensitive fields and fields named like paging inputs are never filters");
        let properties = &tool.input_schema["properties"];
        for (field, schema) in [
            ("id", json!({"type": "string", "format": "uuid"})),
            ("status", json!({"type": "string", "enum": ["open", "closed"]})),
            ("nights", json!({"type": "number"})),
            ("paid", json!({"type": "boolean"})),
            ("arrival", json!({"type": "string", "format": "date"})),
            ("room_id", json!({"type": "string", "format": "uuid"})),
        ] {
            assert_eq!(properties[field], schema, "{field}");
        }
        let sorts = properties["orderBy"]["enum"].as_array().unwrap();
        assert!(!sorts.contains(&json!("passport")), "a sensitive field is never a sort");
        assert!(sorts.contains(&json!("notes")) && sorts.contains(&json!("created_at")));
        assert_eq!(tool.input_schema["additionalProperties"], json!(false));
        assert_eq!(hints(&tool), (Some(true), Some(false), Some(true), Some(false)));
        assert_eq!(security(&tool), Some(&json!([{"type": "oauth2", "scopes": ["mcp:read"]}])));
    }

    #[test]
    fn a_list_call_passes_only_declared_inputs_to_the_read() {
        let filters = vec!["status".to_string(), "paid".to_string()];
        let input = |value: JsonValue| value.as_object().cloned().unwrap();
        assert_eq!(
            query("bookings", &filters, input(json!({"status": "open", "paid": false, "orderBy": "guest", "limit": 500}))).unwrap(),
            json!({"entity": "bookings", "where": {"status": "open", "paid": false}, "orderBy": "guest", "limit": MAX_LIMIT}),
        );
        assert_eq!(query("bookings", &filters, JsonObject::new()).unwrap(), json!({"entity": "bookings", "where": {}, "limit": DEFAULT_LIMIT}));
        for (label, bad) in [
            ("operator smuggled in a filter", json!({"status": {"$ne": "open"}})),
            ("array value", json!({"status": ["open"]})),
            ("field that is not a filter", json!({"passport": "X"})),
            ("raw where clause", json!({"where": {}})),
            ("negative limit", json!({"limit": -1})),
        ] {
            assert!(query("bookings", &filters, input(bad)).is_err(), "{label}");
        }
    }

    #[test]
    fn the_tools_of_an_app_follow_permissions_and_never_share_a_name() {
        let actions = json!([
            {"id": "lookup", "name": "Look up"},
            {"id": "list_rooms", "name": "Rooms report"},
            {"id": "rooms__list", "name": "Rooms export"},
            {"id": "send", "name": "Send", "humanOnly": true}
        ]);
        let entities: Vec<EntityContract> = serde_json::from_value(json!([
            {"entityName": "bookings", "fields": [{"name": "guest", "type": "text"}]},
            {"entityName": "rooms", "fields": [{"name": "label", "type": "text"}]},
            {"entityName": "secrets", "fields": [{"name": "code", "type": "text"}]},
            {"entityName": "booking_index", "derivedFrom": "bookings", "fields": [{"name": "guest", "type": "text"}]}
        ])).unwrap();
        let permissions: Vec<String> = ["app:desk:invoke", "tool:query_data", "app:desk:bookings.read.own", "app:desk:rooms.read", "app:desk:booking_index.read"]
            .map(String::from).to_vec();
        let mut names: Vec<String> = unique(app_tools("desk", "Front desk", &actions, &entities, &permissions)).into_keys().collect();
        names.sort();
        assert_eq!(names, ["desk__bookings__list", "desk__list_rooms", "desk__lookup"],
            "the action rooms__list and the read of rooms share a name, so neither is offered; \
             no tool for a people-only action, an unreadable entity or a derived one");

        for (label, granted, expected) in [
            ("without the invoke grant there is no action, so no collision", vec!["tool:query_data", "app:desk:rooms.read"], vec!["desk__rooms__list"]),
            ("without the read tool grant there is no read, so the action rooms__list stands alone",
                vec!["app:desk:invoke", "app:desk:rooms.read"], vec!["desk__list_rooms", "desk__lookup", "desk__rooms__list"]),
        ] {
            let granted: Vec<String> = granted.into_iter().map(String::from).collect();
            let mut names: Vec<String> = unique(app_tools("desk", "Front desk", &actions, &entities, &granted)).into_keys().collect();
            names.sort();
            assert_eq!(names, expected, "{label}");
        }
    }

    #[test]
    fn ids_that_cannot_name_a_tool_get_none() {
        let longest = "a".repeat(122);
        for (operation, expected) in [
            ("lookup".to_string(), Some("desk__lookup".to_string())),
            ("look up".to_string(), None),
            ("réserver".to_string(), None),
            (format!("{longest}a"), None),
            (longest.clone(), Some(format!("desk__{longest}"))),
        ] {
            assert_eq!(tool_name("desk", &operation), expected, "{operation}");
        }
    }
}
