use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, LayerFuture, ScaffoldContext};

const SCAFFOLD_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self' https: http://localhost:* http://127.0.0.1:*; img-src 'self' data:";

const ICON_PNG: &[u8] = include_bytes!("../../../icons/32x32.png");
const ICON_ICO: &[u8] = include_bytes!("../../../icons/icon.ico");
const ICON_ICNS: &[u8] = include_bytes!("../../../icons/icon.icns");

/// Emits: src-tauri/* (Cargo.toml, tauri.conf.json, lib.rs, main.rs, icons, capabilities)
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
rootcx-client = "0.6"
tokio = {{ version = "1", features = ["net", "io-util", "time"] }}
urlencoding = "2"
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
                    r#"use std::collections::HashMap;
use tauri_plugin_shell::ShellExt;
use tokio::io::{{AsyncReadExt, AsyncWriteExt}};
use tokio::net::TcpListener;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct OidcTokens {{ access_token: String, refresh_token: String }}

#[tauri::command]
async fn oidc_login(app: tauri::AppHandle, provider_id: String) -> Result<OidcTokens, String> {{
    let core_url = std::env::var("VITE_ROOTCX_URL").unwrap_or_else(|_| "http://localhost:9100".into());
    let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let cb = format!("http://127.0.0.1:{{port}}/callback");
    let url = format!("{{}}/api/v1/auth/oidc/{{}}/authorize?redirect_uri={{}}", core_url, urlencoding::encode(&provider_id), urlencoding::encode(&cb));
    app.shell().open(&url, None).map_err(|e| e.to_string())?;
    tokio::time::timeout(std::time::Duration::from_secs(120), accept_callback(listener))
        .await.map_err(|_| "OIDC login timed out".to_string())?
}}

async fn accept_callback(listener: TcpListener) -> Result<OidcTokens, String> {{
    let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
    drop(listener);
    let mut buf = vec![0u8; 8192];
    let mut n = 0;
    loop {{
        let r = stream.read(&mut buf[n..]).await.map_err(|e| e.to_string())?;
        if r == 0 {{ break; }} n += r;
        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {{ break; }}
        if n >= buf.len() {{ break; }}
    }}
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).ok_or("bad request")?;
    let q = path.split('?').nth(1).unwrap_or("");
    let mut p: HashMap<String, String> = q.split('&').filter_map(|pair| {{
        let mut kv = pair.splitn(2, '=');
        Some((kv.next()?.into(), urlencoding::decode(kv.next()?).ok()?.into()))
    }}).collect();
    let html = |ok: bool| format!("HTTP/1.1 {{}}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<h3>{{}}</h3>",
        if ok {{ "200 OK" }} else {{ "400 Bad Request" }}, if ok {{ "Authenticated — you can close this tab." }} else {{ "Authentication failed." }});
    let res = match (p.remove("access_token"), p.remove("refresh_token")) {{
        (Some(at), Some(rt)) => {{ let _ = stream.write_all(html(true).as_bytes()).await; Ok(OidcTokens {{ access_token: at, refresh_token: rt }}) }}
        _ => {{ let _ = stream.write_all(html(false).as_bytes()).await; Err("missing tokens".into()) }}
    }};
    let _ = stream.shutdown().await;
    res
}}

pub fn run() {{
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![oidc_login])
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
