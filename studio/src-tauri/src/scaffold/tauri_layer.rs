use rootcx_scaffold::emitter::Emitter;
use rootcx_scaffold::types::{Layer, LayerFuture, ScaffoldContext};

const SCAFFOLD_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' https: http://localhost:* http://127.0.0.1:*; img-src 'self' data:";

const ICON_PNG: &[u8] = include_bytes!("../../icons/32x32.png");
const ICON_ICO: &[u8] = include_bytes!("../../icons/icon.ico");
const ICON_ICNS: &[u8] = include_bytes!("../../icons/icon.icns");

pub struct TauriLayer;

impl Layer for TauriLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let ScaffoldContext { app_id, identifier, port, .. } = ctx;

            e.write_bytes("src-tauri/icons/icon.png", ICON_PNG).await?;
            e.write_bytes("src-tauri/icons/icon.ico", ICON_ICO).await?;
            e.write_bytes("src-tauri/icons/icon.icns", ICON_ICNS).await?;

            e.write(
                "src-tauri/Cargo.toml",
                &format!(
                    r#"[package]
name = "{app_id}"
version = "0.0.1"
edition = "2021"

[lib]
name = "{app_id}_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = {{ version = "2", features = [] }}

[dependencies]
tauri = {{ version = "2", features = [] }}
tauri-plugin-shell = "2"
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
rootcx-client = {{ version = "0.11", features = ["tauri"] }}
"#
                ),
            )
            .await?;

            e.write("src-tauri/build.rs", "fn main() { tauri_build::build(); }\n").await?;

            e.write(
                "src-tauri/tauri.conf.json",
                &format!(
                    r#"{{
  "productName": "{app_id}",
  "version": "0.0.1",
  "identifier": "{identifier}",
  "build": {{
    "beforeDevCommand": "npm run dev",
    "devUrl": "http://localhost:{port}",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  }},
  "app": {{
    "windows": [{{ "title": "{app_id}", "width": 900, "height": 600 }}],
    "security": {{ "csp": "{SCAFFOLD_CSP}" }},
    "withGlobalTauri": true
  }},
  "bundle": {{
    "active": true,
    "icon": ["icons/icon.png", "icons/icon.ico", "icons/icon.icns"]
  }}
}}
"#
                ),
            )
            .await?;

            e.write(
                "src-tauri/capabilities/default.json",
                r#"{
  "identifier": "default",
  "description": "Default capabilities",
  "windows": ["main"],
  "permissions": ["core:default", "shell:allow-open"]
}
"#,
            )
            .await?;

            e.write(
                "src-tauri/src/lib.rs",
                &format!(
                    r#"pub fn run() {{
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![rootcx_client::oidc::oidc_login])
        .run(tauri::generate_context!())
        .expect("error while running application");
}}
"#
                ),
            )
            .await?;

            e.write(
                "src-tauri/src/main.rs",
                &format!(
                    r#"#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {{
    {app_id}_lib::run();
}}
"#
                ),
            )
            .await?;

            // Patch package.json with Tauri-specific deps (on top of CoreLayer's base)
            e.merge_json("package.json", &serde_json::json!({
                "scripts": { "tauri": "tauri" },
                "dependencies": { "@tauri-apps/plugin-shell": "^2.0.0" },
                "devDependencies": { "@tauri-apps/cli": "^2.0.0" }
            })).await?;

            // Patch launch.json with Tauri dev command
            e.merge_json(".rootcx/launch.json", &serde_json::json!({
                "command": "cargo tauri dev"
            })).await?;

            // Append Tauri-specific gitignore entries
            e.append(".gitignore", "src-tauri/vendor/\nsrc-tauri/resources/\n").await?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_csp_enforces_restrictions() {
        assert!(SCAFFOLD_CSP.contains("default-src 'self'"));
        assert!(SCAFFOLD_CSP.contains("script-src 'self'"));
        assert!(!SCAFFOLD_CSP.contains("unsafe-eval"));
    }

    #[test]
    fn studio_csp_enforces_restrictions() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().expect("csp must not be null");
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("script-src 'self'"));
        assert!(!csp.contains("unsafe-eval"));
    }
}
