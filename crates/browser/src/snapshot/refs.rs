use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefEntry {
    pub role: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_node_id: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefRegistry(HashMap<u32, RefEntry>);

impl RefRegistry {
    pub fn insert(&mut self, id: u32, role: String, name: String, selector: Option<String>, backend_node_id: Option<i64>) {
        self.0.insert(id, RefEntry { role, name, selector, backend_node_id });
    }

    pub fn get(&self, id: u32) -> Option<&RefEntry> { self.0.get(&id) }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
    pub fn len(&self) -> usize { self.0.len() }
}
