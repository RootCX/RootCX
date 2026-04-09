use serde::Serialize;
use std::path::PathBuf;
use tokio::fs;

pub struct Emitter {
    root: PathBuf,
}

impl Emitter {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn write(&self, rel: &str, content: &str) -> Result<(), String> {
        self.write_bytes(rel, content.as_bytes()).await
    }

    pub async fn write_bytes(&self, rel: &str, content: &[u8]) -> Result<(), String> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        fs::write(&path, content).await.map_err(|e| format!("write {}: {e}", path.display()))
    }

    pub async fn write_json<T: Serialize>(&self, rel: &str, val: &T) -> Result<(), String> {
        let json = serde_json::to_string_pretty(val).map_err(|e| format!("json: {e}"))?;
        self.write(rel, &json).await
    }

    /// Deep-merge `patch` into an existing JSON file. Creates the file if absent.
    pub async fn merge_json(&self, rel: &str, patch: &serde_json::Value) -> Result<(), String> {
        let path = self.root.join(rel);
        let mut base = if path.exists() {
            let raw = fs::read_to_string(&path).await.map_err(|e| format!("read {}: {e}", path.display()))?;
            serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?
        } else {
            serde_json::Value::Object(Default::default())
        };
        json_merge(&mut base, patch);
        let out = serde_json::to_string_pretty(&base).map_err(|e| format!("json: {e}"))?;
        self.write(rel, &out).await
    }

    /// Append lines to a file, creating it if absent.
    pub async fn append(&self, rel: &str, content: &str) -> Result<(), String> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
        }
        let mut existing = fs::read_to_string(&path).await.unwrap_or_default();
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(content);
        fs::write(&path, existing).await.map_err(|e| format!("write {}: {e}", path.display()))
    }
}

fn json_merge(base: &mut serde_json::Value, patch: &serde_json::Value) {
    match (base, patch) {
        (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                json_merge(b.entry(k.clone()).or_insert(serde_json::Value::Null), v);
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}
