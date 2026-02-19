use serde::Serialize;
use std::path::PathBuf;
use tokio::fs;

/// Thin wrapper over tokio::fs that resolves paths relative to a root.
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
}
