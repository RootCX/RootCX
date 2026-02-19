use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, LayerFuture, ScaffoldContext};

const ICON: &[u8] = include_bytes!("../../../icons/32x32.png");

/// Emits: src-tauri/* (Cargo.toml, tauri.conf.json, lib.rs, main.rs, icons, capabilities)
pub struct TauriLayer;

impl Layer for TauriLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let ScaffoldContext { name, app_id, lib_name, identifier, port, .. } = ctx;
            let client_dep = ctx.runtime.client_crate.display();

            e.write_bytes("src-tauri/icons/icon.png", ICON).await?;

            e.write("src-tauri/Cargo.toml", &format!(r#"[package]
name = "{app_id}"
version = "0.0.1"
edition = "2021"

[lib]
name = "{lib_name}_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = {{ version = "2", features = [] }}

[dependencies]
tauri = {{ version = "2", features = [] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
reqwest = {{ version = "0.12", default-features = false, features = ["rustls-tls", "blocking"] }}
rootcx-runtime-client = {{ path = "{client_dep}" }}
"#)).await?;

            e.write("src-tauri/build.rs", "fn main() { tauri_build::build(); }\n").await?;

            e.write("src-tauri/tauri.conf.json", &format!(r#"{{
  "productName": "{name}",
  "version": "0.0.1",
  "identifier": "{identifier}",
  "build": {{
    "beforeDevCommand": "npm install && npm run dev",
    "devUrl": "http://localhost:{port}",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  }},
  "app": {{
    "windows": [{{ "title": "{name}", "width": 900, "height": 600 }}],
    "security": {{ "csp": null }}
  }},
  "bundle": {{ "active": true, "icon": ["icons/icon.png"] }}
}}
"#)).await?;

            e.write("src-tauri/capabilities/default.json", r#"{
  "identifier": "default",
  "description": "Default capabilities",
  "windows": ["main"],
  "permissions": ["core:default"]
}
"#).await?;

            e.write("src-tauri/src/lib.rs", r#"pub fn run() {
    rootcx_runtime_client::ensure_runtime().expect("failed to start RootCX Runtime");

    let _ = reqwest::blocking::Client::new()
        .post("http://localhost:9100/api/v1/apps")
        .header("Content-Type", "application/json")
        .body(include_str!("../../manifest.json"))
        .send();

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running application");
}
"#).await?;

            e.write("src-tauri/src/main.rs", &format!(r#"#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {{
    {lib_name}_lib::run();
}}
"#)).await?;

            Ok(())
        })
    }
}
