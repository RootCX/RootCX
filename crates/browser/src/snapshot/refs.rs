use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefEntry {
    pub selector: String,
    pub role: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefRegistry {
    entries: HashMap<u32, RefEntry>,
}

impl RefRegistry {
    pub fn insert(&mut self, id: u32, selector: String, role: String, name: String) {
        self.entries.insert(id, RefEntry { selector, role, name });
    }

    pub fn get(&self, ref_id: u32) -> Option<&RefEntry> {
        self.entries.get(&ref_id)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
