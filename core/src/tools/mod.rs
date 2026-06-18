pub mod call_action;
pub mod call_integration;
pub mod cli;
pub mod describe_app;
pub mod invoke_agent;
pub mod list_actions;
pub mod list_apps;
pub mod list_integrations;
pub mod mutate_data;
pub mod query_data;
pub mod routes;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;
use rootcx_types::ToolDescriptor;

#[async_trait]
pub trait AgentDispatcher: Send + Sync {
    async fn dispatch(
        &self, pool: &PgPool, caller: &str, target: &str, message: &str,
        parent_tx: Option<tokio::sync::mpsc::Sender<crate::worker::AgentEvent>>,
        invoker_user_id: Option<Uuid>,
        parent_perms: Vec<String>,
        task_scope: Option<Vec<String>>,
    ) -> Result<String, String>;
}

#[async_trait]
pub trait IntegrationCaller: Send + Sync {
    /// `app_id`: the calling app, threaded into credential resolution so
    /// (app × user) and app-wide bindings can select the connection.
    /// `caller`: the RLS identity the sub-worker runs under. `None` lands on the
    /// anonymous worker (RLS denies every row) — callers pass `Some` to act as a
    /// real user, fail-closed when that user is disabled.
    async fn call(
        &self, pool: &PgPool, user_id: Uuid, app_id: Option<&str>,
        integration_id: &str, action_id: &str, input: JsonValue,
        caller: Option<crate::ipc::RpcCaller>,
    ) -> Result<JsonValue, String>;
}

#[async_trait]
pub trait ActionCaller: Send + Sync {
    async fn call(
        &self, app_id: &str, action_id: &str, input: JsonValue, user_id: Uuid,
        caller_app_id: &str, effective_perms: Option<Vec<String>>,
    ) -> Result<JsonValue, String>;
}

pub struct ToolContext {
    pub pool: PgPool,
    pub app_id: String,
    pub user_id: Uuid,
    pub invoker_user_id: Option<Uuid>,
    pub permissions: Vec<String>,
    pub task_scope: Option<Vec<String>>,
    pub args: JsonValue,
    pub agent_dispatch: Option<Arc<dyn AgentDispatcher>>,
    pub integration_caller: Option<Arc<dyn IntegrationCaller>>,
    pub action_caller: Option<Arc<dyn ActionCaller>>,
    pub stream_tx: Option<tokio::sync::mpsc::Sender<crate::worker::AgentEvent>>,
}

pub fn check_permission(permissions: &[String], required: &str) -> Result<(), String> {
    if crate::governance::authority::has_permission(permissions, required) { Ok(()) }
    else { Err(format!("permission denied: {required}")) }
}

#[derive(Debug)]
pub enum DispatchError {
    PermissionDenied(String),
    ExecutionFailed(String),
}

pub struct ToolOutcome {
    pub value: Result<JsonValue, DispatchError>,
    pub duration_ms: u64,
}

/// The single executor-agnostic path for running one tool under an authority.
/// Gates `tool:{name}` (zero-alloc), then executes. No delivery side effects.
pub async fn dispatch(tool_name: &str, tool: Arc<dyn Tool>, ctx: &ToolContext) -> ToolOutcome {
    let start = Instant::now();
    let perm_key = format!("tool:{tool_name}");
    let value = if !crate::governance::authority::has_permission(&ctx.permissions, &perm_key) {
        Err(DispatchError::PermissionDenied(perm_key))
    } else {
        tool.execute(ctx).await.map_err(DispatchError::ExecutionFailed)
    };
    ToolOutcome { value, duration_ms: start.elapsed().as_millis() as u64 }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    fn enriches_with_schema(&self) -> bool { false }
    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String>;
}

pub struct ToolRegistry {
    tools: RwLock<HashMap<String, (Arc<dyn Tool>, ToolDescriptor)>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self { tools: RwLock::new(HashMap::new()) }
    }
}

impl ToolRegistry {
    pub fn register(&self, tool: impl Tool + 'static) {
        let desc = tool.descriptor();
        let name = desc.name.clone();
        self.tools.write().unwrap().insert(name, (Arc::new(tool), desc));
    }

    pub fn unregister(&self, name: &str) {
        self.tools.write().unwrap().remove(name);
    }

    pub fn unregister_prefix(&self, prefix: &str) {
        self.tools.write().unwrap().retain(|name, _| !name.starts_with(prefix));
    }

    /// Sync all current tool names into rbac_permissions (global).
    pub async fn sync_to_db(&self, pool: &PgPool) {
        let (names, descs): (Vec<String>, Vec<String>) = self.tools.read().unwrap()
            .values().map(|(_, d)| (format!("tool:{}", d.name), d.description.clone())).unzip();
        let mut tx = match pool.begin().await {
            Ok(tx) => tx,
            Err(e) => { tracing::warn!("tool sync begin: {e}"); return; }
        };
        let _ = sqlx::query("DELETE FROM rootcx_system.rbac_permissions WHERE key LIKE 'tool:%'")
            .execute(&mut *tx).await;
        if !names.is_empty() {
            let _ = sqlx::query(
                "INSERT INTO rootcx_system.rbac_permissions (key, description)
                 SELECT unnest($1::text[]), unnest($2::text[])
                 ON CONFLICT (key) DO UPDATE SET description = EXCLUDED.description"
            ).bind(&names).bind(&descs).execute(&mut *tx).await;
        }
        if let Err(e) = tx.commit().await {
            tracing::warn!("tool sync commit: {e}");
        }
    }

    pub fn descriptors_for_permissions(&self, permissions: &[String], data_contract: &JsonValue) -> Vec<ToolDescriptor> {
        self.tools.read().unwrap().values().filter_map(|(tool, base)| {
            let perm = format!("tool:{}", base.name);
            if !crate::governance::authority::has_permission(permissions, &perm) { return None; }
            let mut desc = base.clone();
            if tool.enriches_with_schema() {
                desc.description.push_str(&format_data_contract(data_contract));
            }
            Some(desc)
        }).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().unwrap().get(name).map(|(t, _)| t.clone())
    }

    pub fn all_summaries(&self) -> Vec<(String, String)> {
        let mut out: Vec<_> = self.tools.read().unwrap().values()
            .map(|(_, d)| (d.name.clone(), d.description.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

pub(crate) fn str_arg<'a>(args: &'a JsonValue, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(|v| v.as_str()).ok_or_else(|| format!("missing: {key}"))
}

fn format_data_contract(contract: &JsonValue) -> String {
    let Some(entities) = contract.as_array().filter(|a| !a.is_empty()) else { return String::new() };
    let lines: Vec<String> = entities.iter().filter_map(|e| {
        let name = e.get("entityName")?.as_str()?;
        let fields: Vec<String> = e.get("fields")?.as_array()?.iter().filter_map(|f| {
            let fname = f.get("name")?.as_str()?;
            let ftype = f.get("type")?.as_str()?;
            let mut s = format!("{fname}({ftype}");
            if f.get("required").and_then(|v| v.as_bool()) == Some(true) { s.push_str(", required"); }
            if let Some(vals) = f.get("enum_values").and_then(|v| v.as_array()) {
                let v: Vec<&str> = vals.iter().filter_map(|v| v.as_str()).collect();
                if !v.is_empty() { s.push_str(&format!(": {}", v.join("|"))); }
            }
            if let Some(re) = f.get("references").and_then(|r| r.get("entity")).and_then(|v| v.as_str()) {
                s.push_str(&format!(" → {re}"));
            }
            s.push(')');
            Some(s)
        }).collect();
        Some(format!("- {name}: {}", fields.join(", ")))
    }).collect();
    format!("\nSchema:\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use uuid::Uuid;
    use async_trait::async_trait;
    use rootcx_types::ToolDescriptor;

    struct StubTool(Result<serde_json::Value, String>);
    #[async_trait]
    impl Tool for StubTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor { name: "ok".into(), description: String::new(), input_schema: json!({}) }
        }
        async fn execute(&self, _ctx: &ToolContext) -> Result<serde_json::Value, String> {
            self.0.clone()
        }
    }

    fn ctx_with(perms: Vec<String>) -> ToolContext {
        // Lazy pool never connects: the gate/lookup cases short-circuit before
        // any query, and the stub tool ignores ctx.pool.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/test").unwrap();
        ToolContext {
            pool, app_id: "app".into(), user_id: Uuid::nil(), invoker_user_id: None,
            permissions: perms, task_scope: None, args: json!({}),
            agent_dispatch: None, integration_caller: None, action_caller: None, stream_tx: None,
        }
    }

    #[tokio::test]
    async fn dispatch_gates_then_maps_outcome() {
        let ok = || -> Arc<dyn Tool> { Arc::new(StubTool(Ok(json!({ "ran": true })))) };
        let failing = || -> Arc<dyn Tool> { Arc::new(StubTool(Err("boom".into()))) };

        // Denied: no perm
        let out = dispatch("ok", ok(), &ctx_with(vec![])).await;
        assert!(matches!(out.value, Err(DispatchError::PermissionDenied(_))), "should deny with no perm");

        // Denied: unrelated perm
        let out = dispatch("ok", ok(), &ctx_with(vec!["tool:other".into()])).await;
        assert!(matches!(out.value, Err(DispatchError::PermissionDenied(_))), "should deny with unrelated perm");

        // Allowed: exact perm
        let out = dispatch("ok", ok(), &ctx_with(vec!["tool:ok".into()])).await;
        assert_eq!(out.value.unwrap(), json!({ "ran": true }));

        // Allowed: wildcard perm
        let out = dispatch("ok", ok(), &ctx_with(vec!["tool:*".into()])).await;
        assert_eq!(out.value.unwrap(), json!({ "ran": true }));

        // Allowed: tool execution error propagates
        let out = dispatch("ok", failing(), &ctx_with(vec!["tool:ok".into()])).await;
        assert!(matches!(out.value, Err(DispatchError::ExecutionFailed(ref e)) if e == "boom"));
    }

    // `enum_values` is read from raw JSON (not type-guarded), so a casing drift
    // back to `enumValues` would silently drop enum options from agent prompts.
    // Mirrors the stored-manifest shape: camelCase `entityName`, snake `enum_values`.
    #[test]
    fn format_data_contract_surfaces_enum_options() {
        let contract = json!([{
            "entityName": "person",
            "fields": [{ "name": "gender", "type": "text", "enum_values": ["male", "female"] }],
        }]);
        assert!(
            format_data_contract(&contract).contains("gender(text: male|female)"),
            "enum options missing from schema description"
        );
    }
}
