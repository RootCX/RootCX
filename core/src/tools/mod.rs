pub mod browser;
pub mod mutate_data;
pub mod query_data;
pub mod routes;

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use rootcx_shared_types::ToolDescriptor;

pub struct ToolContext {
    pub pool: PgPool,
    pub app_id: String,
    pub args: JsonValue,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    fn enriches_with_schema(&self) -> bool { false }
    async fn execute(&self, ctx: &ToolContext) -> Result<JsonValue, String>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, (Arc<dyn Tool>, ToolDescriptor)>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: impl Tool + 'static) {
        let desc = tool.descriptor();
        let name = desc.name.clone();
        self.tools.insert(name, (Arc::new(tool), desc));
    }

    pub fn descriptors_for(&self, names: &[String], data_contract: &JsonValue) -> Vec<ToolDescriptor> {
        names.iter().filter_map(|name| {
            let (tool, base) = self.tools.get(name)?;
            let mut desc = base.clone();
            if tool.enriches_with_schema() {
                desc.description.push_str(&format_data_contract(data_contract));
            }
            Some(desc)
        }).collect()
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name).map(|(t, _)| t)
    }

    pub fn all_summaries(&self) -> Vec<(String, String)> {
        let mut out: Vec<_> = self.tools.values()
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
