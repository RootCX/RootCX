use std::path::Path;

use serde::{Deserialize, Serialize};

const DIR: &str = ".rootcx";
const FILE: &str = "launch.json";
const TEMPLATE: &str = r#"{
  "command": "echo hello world"
}
"#;

#[derive(Serialize, Deserialize)]
pub struct LaunchConfig {
    pub command: String,
}

pub fn read(project: &Path) -> Result<LaunchConfig, String> {
    let path = project.join(DIR).join(FILE);
    let data = std::fs::read_to_string(&path)
        .map_err(|_| format!("{} not found", path.display()))?;
    serde_json::from_str(&data).map_err(|e| format!("invalid launch.json: {e}"))
}

pub fn init(project: &Path) -> Result<(), String> {
    let dir = project.join(DIR);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(FILE);
    if !path.exists() {
        std::fs::write(&path, TEMPLATE).map_err(|e| e.to_string())?;
    }
    Ok(())
}
