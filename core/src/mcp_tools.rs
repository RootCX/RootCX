//! The tools `/mcp` offers on top of its own (ADR 0016): the Core tools a
//! person may call outside agent execution, as the tool registry declares
//! them, and one tool per declared action, `<app>__<action>`. Listing and
//! calling share one decision, [`offered`], so a tool is callable exactly when
//! it is listed, and every call runs under the assistant rules.

use std::collections::BTreeMap;
use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use serde_json::{Value as JsonValue, json};

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::governance::action_invocation::{Declaration, dispatch};
use crate::governance::authority::has_permission;
use crate::mcp_server::{oauth_tool_meta, scopes};
use crate::routes::SharedRuntime;
use crate::tools::list_actions::invocable;

const SEPARATOR: &str = "__";

/// What an offered tool runs.
pub(crate) enum Operation {
    /// A registry tool, run for the person as an assistant.
    Core { name: String, read_only: bool },
    /// A declared action, run through `action_invocation`.
    Action { app: String, action: String, declaration: Declaration },
}

impl Operation {
    pub(crate) fn read_only(&self) -> bool {
        match self {
            Self::Core { read_only, .. } => *read_only,
            Self::Action { declaration, .. } => declaration.read_only,
        }
    }

    /// Runs the operation for the person, through the Modules every other entry uses.
    pub(crate) async fn run(self, runtime: &SharedRuntime, identity: &Identity, input: JsonObject) -> Result<JsonValue, String> {
        match self {
            Self::Core { name, .. } => {
                let app = input.get("app").and_then(JsonValue::as_str).unwrap_or_default().to_string();
                crate::tools::routes::execute_as_person(runtime, identity, &name, app, JsonValue::Object(input), true)
                    .await
                    .map_err(|error| error.to_string())
            }
            Self::Action { app, action, declaration } => {
                let caller = crate::ipc::RpcCaller::person(identity.user_id, &identity.email);
                let dispatcher = runtime.worker_manager().action_caller();
                dispatch(dispatcher.as_ref(), &app, &action, Some(&declaration), JsonValue::Object(input), caller)
                    .await
                    .map_err(|error| error.to_string())
            }
        }
    }
}

/// The tools this person may call.
pub(crate) async fn list(runtime: &SharedRuntime, permissions: &[String]) -> Result<Vec<Tool>, ApiError> {
    Ok(offered(runtime, permissions).await?.into_values().map(|(tool, _)| tool).collect())
}

/// The operation behind an offered tool name; `None` for any name [`list`] does not offer.
pub(crate) async fn resolve(runtime: &SharedRuntime, permissions: &[String], name: &str) -> Result<Option<Operation>, ApiError> {
    Ok(offered(runtime, permissions).await?.remove(name).map(|(_, operation)| operation))
}

/// Every tool offered to this person, by name, in name order.
async fn offered(runtime: &SharedRuntime, permissions: &[String]) -> Result<BTreeMap<String, (Tool, Operation)>, ApiError> {
    let core = runtime
        .tool_registry()
        .http_descriptors()
        .into_iter()
        .filter(|(descriptor, _)| has_permission(permissions, &format!("tool:{}", descriptor.name)))
        .filter_map(|(descriptor, hints)| core_tool(descriptor, hints));
    let apps: Vec<(String, String, JsonValue)> = sqlx::query_as(
        "SELECT id, name, COALESCE(manifest->'actions', '[]'::jsonb)
           FROM rootcx_system.apps WHERE status = 'installed' ORDER BY id",
    )
    .fetch_all(runtime.pool())
    .await?;
    let actions = apps.iter().flat_map(|(app, app_name, actions)| app_actions(app, app_name, actions, permissions));
    Ok(unique(core.chain(actions)))
}

/// A registry tool as it declares itself.
fn core_tool(descriptor: rootcx_types::ToolDescriptor, hints: Option<rootcx_types::ToolAnnotations>) -> Option<(String, Tool, Operation)> {
    let input = standalone_object_schema(&descriptor.input_schema)?;
    let hints = hints.unwrap_or_default();
    let read_only = hints.read_only_hint == Some(true);
    let annotations = annotations(read_only, hints.destructive_hint, hints.idempotent_hint, hints.open_world_hint);
    let tool = tool(&descriptor.name, None, descriptor.description, input, json!({}), annotations, read_only);
    Some((descriptor.name.clone(), tool, Operation::Core { name: descriptor.name, read_only }))
}

/// One app's actions an assistant may run, each as its own tool.
fn app_actions(app: &str, app_name: &str, actions: &JsonValue, permissions: &[String]) -> Vec<(String, Tool, Operation)> {
    invocable(actions, app, permissions, true)
        .filter_map(|(id, declaration)| {
            let (name, tool) = action_tool(app, app_name, id, &declaration)?;
            Some((name, tool, Operation::Action { app: app.to_string(), action: id.to_string(), declaration }))
        })
        .collect()
}

/// The tools by name. A name two operations would share is offered for neither,
/// so a call is never ambiguous.
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
fn tool_name(app: &str, action: &str) -> Option<String> {
    let name = format!("{app}{SEPARATOR}{action}");
    let valid = name.len() <= 128 && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'));
    valid.then_some(name)
}

/// The action as its manifest declares it; an approved action is never read-only.
fn action_tool(app: &str, app_name: &str, id: &str, declaration: &Declaration) -> Option<(String, Tool)> {
    let name = tool_name(app, id)?;
    let action = &declaration.definition;
    let text = |key: &str| action.get(key).and_then(JsonValue::as_str).map(str::trim).filter(|value| !value.is_empty());
    let label = text("name").unwrap_or(id);
    let input = match action.get("inputSchema") {
        None | Some(JsonValue::Null) => object(json!({ "type": "object" })),
        Some(schema) => match standalone_object_schema(schema) {
            Some(schema) => schema,
            None => {
                // One unusable schema must not make a host refuse every other tool.
                tracing::warn!(app, action = id, "input schema is not a self-contained object schema: no tool");
                return None;
            }
        },
    };
    let hint = |key: &str| action.get("annotations").and_then(|hints| hints.get(key)).and_then(JsonValue::as_bool);
    let read_only = declaration.read_only;
    let annotations = annotations(read_only, hint("destructiveHint"), hint("idempotentHint"), hint("openWorldHint"));
    let description = format!("{app_name}: {}", text("description").unwrap_or(label));
    let result = action.get("outputSchema").filter(|schema| schema.is_object()).cloned().unwrap_or_else(|| json!({}));
    let tool = tool(&name, Some(format!("{label} ({app_name})")), description, input, result, annotations, read_only);
    Some((name, tool))
}

/// Explicit hints for every tool: what is declared, the worst case for the
/// rest. A read never destroys and is idempotent unless it says otherwise.
fn annotations(read_only: bool, destructive: Option<bool>, idempotent: Option<bool>, open_world: Option<bool>) -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(!read_only && destructive.unwrap_or(true))
        .idempotent(idempotent.unwrap_or(read_only))
        .open_world(open_world.unwrap_or(true))
}

/// Every offered tool returns `{ result }`, the scopes its hints call for, and its title in both places hosts read it.
fn tool(name: &str, title: Option<String>, description: String, input: JsonObject, result: JsonValue, mut annotations: ToolAnnotations, read_only: bool) -> Tool {
    let output = object(json!({ "type": "object", "properties": { "result": result }, "required": ["result"] }));
    let mut tool = Tool::new(name.to_string(), description, Arc::new(input)).with_raw_output_schema(Arc::new(output));
    if let Some(title) = title {
        annotations.title = Some(title.clone());
        tool = tool.with_title(title);
    }
    tool.with_annotations(annotations).with_meta(oauth_tool_meta(scopes(read_only)))
}

/// The schema when a host can read it alone: an object with `type: object` at
/// the root, every `$ref` pointing inside the schema itself.
fn standalone_object_schema(schema: &JsonValue) -> Option<JsonObject> {
    fn refs_resolve(value: &JsonValue, root: &JsonValue) -> bool {
        match value {
            JsonValue::Object(map) => map.iter().all(|(key, item)| match (key.as_str(), item) {
                ("$ref", JsonValue::String(pointer)) => pointer.strip_prefix('#').is_some_and(|path| root.pointer(path).is_some()),
                ("$ref", _) => false,
                _ => refs_resolve(item, root),
            }),
            JsonValue::Array(items) => items.iter().all(|item| refs_resolve(item, root)),
            _ => true,
        }
    }
    let object = schema.as_object()?;
    (object.get("type") == Some(&json!("object")) && refs_resolve(schema, schema)).then(|| object.clone())
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
        assert_eq!(lookup.title.as_deref(), Some("Look up (Front desk)"));
        assert_eq!(lookup.description.as_deref(), Some("Front desk: Finds a booking."));
        assert_eq!(JsonValue::Object((*lookup.input_schema).clone()), json!({"type": "object", "required": ["q"]}));

        let bare = action(json!({"id": "go", "description": ""})).unwrap();
        assert_eq!(bare.description.as_deref(), Some("Front desk: go"), "an empty description falls back to the name, then the id");
        assert_eq!(JsonValue::Object((*bare.input_schema).clone()), json!({"type": "object"}), "no declared schema: an empty object");
    }

    #[test]
    fn a_schema_that_cannot_stand_alone_gives_no_tool() {
        let local_refs = json!({"type": "object", "properties": {"line": {"$ref": "#/$defs/line"}}, "$defs": {"line": {"type": "object"}}});
        assert!(action(json!({"id": "go", "inputSchema": local_refs})).is_some(), "refs inside the schema resolve");
        for (label, schema) in [
            ("ref to another part of the manifest", json!({"$ref": "#/actions/0/inputSchema"})),
            ("nested ref that resolves nowhere", json!({"type": "object", "properties": {"a": {"$ref": "#/$defs/missing"}}})),
            ("remote ref", json!({"type": "object", "properties": {"a": {"$ref": "https://example.test/a.json"}}})),
            ("root that is not an object", json!({"type": "array"})),
            ("not a schema", json!("not a schema")),
        ] {
            assert!(action(json!({"id": "go", "inputSchema": schema})).is_none(), "{label}");
        }
    }

    #[test]
    fn a_core_tool_carries_the_registry_s_hints_and_scope() {
        let descriptor = |name: &str| rootcx_types::ToolDescriptor { name: name.into(), description: "d".into(), input_schema: json!({"type": "object"}) };
        for (label, declared, expected, scopes) in [
            ("declared read", Some(rootcx_types::ToolAnnotations::new(true, false, true, false)), (true, false, true, false), vec!["mcp:read"]),
            ("declared write", Some(rootcx_types::ToolAnnotations::new(false, true, false, false)), (false, true, false, false), vec!["mcp:read", "mcp:write"]),
            ("no hints: worst case", None, (false, true, false, true), vec!["mcp:read", "mcp:write"]),
        ] {
            let (name, tool, operation) = core_tool(descriptor("query"), declared).unwrap_or_else(|| panic!("{label}"));
            assert_eq!(name, "query");
            assert_eq!(hints(&tool), (Some(expected.0), Some(expected.1), Some(expected.2), Some(expected.3)), "{label}");
            assert_eq!(security(&tool), Some(&json!([{"type": "oauth2", "scopes": scopes}])), "{label}");
            assert_eq!(operation.read_only(), expected.0, "{label}");
        }
    }

    #[test]
    fn a_name_two_operations_share_is_offered_for_neither() {
        let declaration = || Declaration::from_definition(json!({"id": "go"}));
        let candidate = |name: &str| (name.to_string(), action(json!({"id": "go"})).unwrap(), Operation::Action { app: "desk".into(), action: "go".into(), declaration: declaration() });
        let names: Vec<String> = unique([candidate("desk__go"), candidate("desk__go"), candidate("desk__stop")]).into_keys().collect();
        assert_eq!(names, ["desk__stop"]);
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
