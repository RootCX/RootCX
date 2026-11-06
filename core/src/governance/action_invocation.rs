//! The one place that decides whether an app action may run (`authorize`) and
//! how it runs (`dispatch`). The RPC route, the agent `call_action` tool and the
//! MCP `call_action` tool all come through here, so a grant means the same thing
//! on every entry (ADR 0015).

use serde_json::Value as JsonValue;
use sqlx::PgPool;

use crate::api_error::ApiError;
use crate::governance::authority::has_permission;
use crate::ipc::RpcCaller;
use crate::tools::ActionCaller;

/// What the manifest says about one RPC method. `None` from [`Declaration::load`]
/// means the method is not a declared action.
pub struct Declaration {
    isolated_scope: bool,
    approved: bool,
    human_only: bool,
    pub read_only: bool,
    /// The declared action, for callers that read its input schema.
    pub definition: JsonValue,
}

impl Declaration {
    pub async fn load(pool: &PgPool, app: &str, method: &str) -> Result<Option<Self>, ApiError> {
        let row: Option<(JsonValue,)> = sqlx::query_as(
            "SELECT action FROM rootcx_system.apps app
               CROSS JOIN LATERAL jsonb_array_elements(COALESCE(app.manifest->'actions', '[]'::jsonb)) action
              WHERE app.id = $1 AND action->>'id' = $2",
        )
        .bind(app)
        .bind(method)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(|(definition,)| Self::from_definition(definition)))
    }

    pub fn from_definition(definition: JsonValue) -> Self {
        let flag = |pointer: &str| definition.pointer(pointer).and_then(JsonValue::as_bool) == Some(true);
        Self {
            isolated_scope: flag("/isolatedScope"),
            approved: definition.get("authority").is_some_and(|a| !a.is_null()),
            human_only: flag("/humanOnly"),
            // An approved action carries data authority: never a read, whatever its hint.
            read_only: flag("/annotations/readOnlyHint") && !definition.get("authority").is_some_and(|a| !a.is_null()),
            definition,
        }
    }
}

/// Dispatches a call [`authorize`] already admitted. A dedicated process is posed
/// only for an action that asked for one; every other call shares its caller's
/// worker with empty invocation settings.
pub async fn dispatch(
    dispatcher: &dyn ActionCaller,
    app: &str,
    method: &str,
    declaration: Option<&Declaration>,
    params: JsonValue,
    caller: RpcCaller,
) -> Result<JsonValue, ApiError> {
    let scoped = declaration.is_some_and(|d| d.isolated_scope || d.approved);
    dispatcher.call(app, method, params, caller, scoped).await.map_err(ApiError::from)
}

/// The single decision for calls and listings. Grants come first, so a caller
/// without them learns nothing about how the app declares its actions.
pub(crate) fn authorize(app: &str, method: &str, declaration: Option<&Declaration>, permissions: &[String], assistant: bool) -> Result<(), ApiError> {
    // Two grains of one right: `invoke` is the whole app, `action:{id}` that
    // action alone. An approved action holds more data authority than its caller
    // (ADR 0008), so only its own grain may authorize it.
    let invoke_key = format!("app:{app}:invoke");
    let action_key = format!("app:{app}:action:{method}");
    let holds = |key: &str| has_permission(permissions, key);
    let (allowed, needed) = match declaration {
        Some(d) if d.approved => (holds(&action_key), action_key.clone()),
        Some(_) => (holds(&invoke_key) || holds(&action_key), format!("{invoke_key} or {action_key}")),
        None => (holds(&invoke_key), invoke_key.clone()),
    };
    if !allowed {
        return Err(ApiError::Forbidden(format!("permission denied: {needed}")));
    }
    match declaration {
        None if assistant => Err(ApiError::NotFound(format!("action '{method}' not found in app '{app}'"))),
        Some(d) if assistant && d.human_only => Err(ApiError::Forbidden(format!("action '{method}' is for people only"))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn declared(approved: bool, human_only: bool) -> Declaration {
        Declaration { isolated_scope: false, approved, human_only, read_only: false, definition: json!({}) }
    }

    #[test]
    fn grains_decide_by_declaration() {
        let invoke = vec!["app:a:invoke".to_string()];
        let fine = vec!["app:a:action:go".to_string()];
        for (label, declaration, permissions, assistant, allowed) in [
            ("undeclared, invoke", None, &invoke, false, true),
            ("undeclared, fine only", None, &fine, false, false),
            ("declared, invoke", Some(declared(false, false)), &invoke, false, true),
            ("declared, fine", Some(declared(false, false)), &fine, false, true),
            ("approved, invoke only", Some(declared(true, false)), &invoke, false, false),
            ("approved, fine", Some(declared(true, false)), &fine, false, true),
            ("assistant, undeclared", None, &invoke, true, false),
            ("assistant, undeclared, no grant", None, &fine, true, false),
            ("assistant, humanOnly", Some(declared(false, true)), &invoke, true, false),
            ("person, humanOnly", Some(declared(false, true)), &invoke, false, true),
        ] {
            let outcome = authorize("a", "go", declaration.as_ref(), permissions, assistant);
            assert_eq!(outcome.is_ok(), allowed, "{label}: {outcome:?}");
        }
    }
}
