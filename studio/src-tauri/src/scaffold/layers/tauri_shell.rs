use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, LayerFuture, ScaffoldContext};

const SCAFFOLD_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' http://127.0.0.1:* http://localhost:*; img-src 'self' data:";

const ICON: &[u8] = include_bytes!("../../../icons/32x32.png");

/// Emits: src-tauri/* (Cargo.toml, tauri.conf.json, lib.rs, main.rs, icons, capabilities)
pub struct TauriLayer;

impl Layer for TauriLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let ScaffoldContext { app_id, identifier, port, .. } = ctx;

            e.write_bytes("src-tauri/icons/icon.png", ICON).await?;

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
reqwest = {{ version = "0.12", default-features = false, features = ["rustls-tls", "blocking"] }}
rootcx-client = "0.1"
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
    "beforeDevCommand": "npm install && npm run dev",
    "devUrl": "http://localhost:{port}",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  }},
  "app": {{
    "windows": [{{ "title": "{app_id}", "width": 900, "height": 600 }}],
    "security": {{ "csp": "{SCAFFOLD_CSP}" }}
  }},
  "bundle": {{
    "active": true,
    "icon": ["icons/icon.png"]
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
    match rootcx_client::ensure_runtime() {{
        Ok(rootcx_client::RuntimeStatus::Ready) => {{}}
        Ok(rootcx_client::RuntimeStatus::NotInstalled) => {{
            rootcx_client::prompt_runtime_install()
                .expect("RootCX Runtime installation required");
        }}
        Err(e) => panic!("Failed to start RootCX Runtime: {{e}}"),
    }}

    rootcx_client::deploy_bundled_backend("{app_id}");

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
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
        let conf: serde_json::Value = serde_json::from_str(include_str!("../../../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().expect("csp must not be null");
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("script-src 'self'"));
        assert!(!csp.contains("unsafe-eval"));
    }
}
