use std::collections::HashMap;
use tauri_plugin_shell::ShellExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OidcTokens {
    pub access_token: String,
    pub refresh_token: String,
}

/// Tauri command: OIDC login via system browser + loopback (RFC 8252).
/// Reads `VITE_ROOTCX_URL` env var for Core URL, falls back to `http://localhost:9100`.
#[tauri::command]
pub async fn oidc_login(
    app: tauri::AppHandle,
    provider_id: String,
) -> Result<OidcTokens, String> {
    let core_url = std::env::var("VITE_ROOTCX_URL")
        .unwrap_or_else(|_| "http://localhost:9100".into());

    let listener = TcpListener::bind("127.0.0.1:0").await
        .map_err(|e| format!("failed to bind loopback: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let cb = format!("http://127.0.0.1:{port}/callback");
    let url = format!(
        "{}/api/v1/auth/oidc/{}/authorize?redirect_uri={}",
        core_url,
        urlencoding::encode(&provider_id),
        urlencoding::encode(&cb),
    );

    app.shell().open(&url, None).map_err(|e| format!("failed to open browser: {e}"))?;

    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        accept_callback(listener),
    )
    .await
    .map_err(|_| "OIDC login timed out (120s)".to_string())?
}

async fn accept_callback(listener: TcpListener) -> Result<OidcTokens, String> {
    let (mut stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
    drop(listener);

    let mut buf = vec![0u8; 8192];
    let mut n = 0;
    loop {
        let r = stream.read(&mut buf[n..]).await.map_err(|e| e.to_string())?;
        if r == 0 { break; }
        n += r;
        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") { break; }
        if n >= buf.len() { break; }
    }

    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.lines().next()
        .and_then(|l| l.split_whitespace().nth(1))
        .ok_or("invalid HTTP request")?;

    let q = path.split('?').nth(1).unwrap_or("");
    let mut params: HashMap<String, String> = q.split('&')
        .filter_map(|pair| {
            let mut kv = pair.splitn(2, '=');
            Some((kv.next()?.into(), urlencoding::decode(kv.next()?).ok()?.into()))
        })
        .collect();

    let (status, body) = match (params.remove("access_token"), params.remove("refresh_token")) {
        (Some(at), Some(rt)) => (
            "200 OK",
            Ok(OidcTokens { access_token: at, refresh_token: rt }),
        ),
        _ => (
            "400 Bad Request",
            Err(params.get("error").cloned().unwrap_or_else(|| "missing tokens".into())),
        ),
    };

    let html = if body.is_ok() {
        "Authenticated — you can close this tab."
    } else {
        "Authentication failed."
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<h3>{html}</h3>"
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
    body
}
