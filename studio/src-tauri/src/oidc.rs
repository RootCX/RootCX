use std::collections::HashMap;
use tauri_plugin_shell::ShellExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::info;

use crate::state::AppState;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcTokens {
    pub access_token: String,
    pub refresh_token: String,
}

const SUCCESS_HTML: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Authenticated</title>
<style>body{font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0a0a0a;color:#fafafa}
.c{text-align:center}h1{font-size:1.5rem;margin-bottom:.5rem}p{color:#888;font-size:.875rem}</style></head>
<body><div class="c"><h1>Authenticated</h1><p>You can close this tab and return to Studio.</p></div></body></html>"#;

const ERROR_HTML: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Error</title>
<style>body{font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0a0a0a;color:#fafafa}
.c{text-align:center}h1{font-size:1.5rem;margin-bottom:.5rem;color:#ef4444}p{color:#888;font-size:.875rem}</style></head>
<body><div class="c"><h1>Authentication Failed</h1><p>Please try again from Studio.</p></div></body></html>"#;

#[tauri::command]
pub async fn oidc_login(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    provider_id: String,
) -> Result<OidcTokens, String> {
    let core_url = state.core_url();
    if core_url.is_empty() {
        return Err("not connected to Core".into());
    }

    let listener = TcpListener::bind("127.0.0.1:0").await
        .map_err(|e| format!("failed to bind loopback: {e}"))?;
    let port = listener.local_addr()
        .map_err(|e| format!("failed to get port: {e}"))?.port();

    let callback_url = format!("http://127.0.0.1:{port}/callback");
    let authorize_url = format!(
        "{}/api/v1/auth/oidc/{}/authorize?redirect_uri={}",
        core_url,
        urlencoding::encode(&provider_id),
        urlencoding::encode(&callback_url),
    );

    info!(port, provider_id = %provider_id, "OIDC login: opening system browser");

    app.shell().open(&authorize_url, None)
        .map_err(|e| format!("failed to open browser: {e}"))?;

    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        wait_for_callback(listener),
    )
    .await
    .map_err(|_| "OIDC login timed out (120s)".to_string())?
}

async fn wait_for_callback(listener: TcpListener) -> Result<OidcTokens, String> {
    let (mut stream, _) = listener.accept().await
        .map_err(|e| format!("accept failed: {e}"))?;
    drop(listener);

    let mut buf = vec![0u8; 8192];
    let mut total = 0;
    loop {
        let n = stream.read(&mut buf[total..]).await
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 { break; }
        total += n;
        if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        if total >= buf.len() { break; }
    }

    let request = String::from_utf8_lossy(&buf[..total]);
    let path = request.lines().next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("invalid HTTP request")?;

    let query = path.split('?').nth(1).unwrap_or("");
    let mut params: HashMap<String, String> = query.split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            Some((parts.next()?.to_string(), urlencoding::decode(parts.next()?).ok()?.to_string()))
        })
        .collect();

    let result = match (params.remove("access_token"), params.remove("refresh_token")) {
        (Some(at), Some(rt)) => {
            send_response(&mut stream, "200 OK", SUCCESS_HTML).await;
            info!("OIDC login successful");
            Ok(OidcTokens { access_token: at, refresh_token: rt })
        }
        _ => {
            send_response(&mut stream, "400 Bad Request", ERROR_HTML).await;
            let err = params.get("error").map(|s| s.as_str()).unwrap_or("unknown error");
            Err(format!("OIDC callback error: {err}"))
        }
    };

    let _ = stream.shutdown().await;
    result
}

async fn send_response(stream: &mut tokio::net::TcpStream, status: &str, html: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len(),
    );
    let _ = stream.write_all(response.as_bytes()).await;
}
