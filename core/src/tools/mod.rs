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
    ) -> Result<String, String>;
}

#[async_trait]
pub trait IntegrationCaller: Send + Sync {
    async fn call(
        &self, pool: &PgPool, user_id: Uuid,
        integration_id: &str, action_id: &str, input: JsonValue,
    ) -> Result<JsonValue, String>;
}

#[async_trait]
pub trait ActionCaller: Send + Sync {
    async fn call(
        &self, app_id: &str, action_id: &str, input: JsonValue, user_id: Uuid,
    ) -> Result<JsonValue, String>;
}

pub struct ToolContext {
    pub pool: PgPool,
    pub app_id: String,
    pub user_id: Uuid,
    pub invoker_user_id: Option<Uuid>,
    pub permissions: Vec<String>,
    pub args: JsonValue,
    pub agent_dispatch: Option<Arc<dyn AgentDispatcher>>,
    pub integration_caller: Option<Arc<dyn IntegrationCaller>>,
    pub action_caller: Option<Arc<dyn ActionCaller>>,
    pub stream_tx: Option<tokio::sync::mpsc::Sender<crate::worker::AgentEvent>>,
}

pub fn check_permission(permissions: &[String], required: &str) -> Result<(), String> {
    if crate::extensions::rbac::policy::has_permission(permissions, required) { Ok(()) }
    else { Err(format!("permission denied: {required}")) }
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
            if !crate::extensions::rbac::policy::has_permission(permissions, &perm) { return None; }
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
            if let Some(vals) = f.get("enumValues").and_then(|v| v.as_array()) {
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
