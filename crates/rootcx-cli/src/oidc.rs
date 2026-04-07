use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
}

const SUCCESS_HTML: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Authenticated</title>
<style>body{font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0a0a0a;color:#fafafa}
.c{text-align:center}h1{font-size:1.5rem;margin-bottom:.5rem}p{color:#888;font-size:.875rem}</style></head>
<body><div class="c"><h1>Authenticated</h1><p>You can close this tab and return to your terminal.</p></div></body></html>"#;

const ERROR_HTML: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Error</title>
<style>body{font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#0a0a0a;color:#fafafa}
.c{text-align:center}h1{font-size:1.5rem;margin-bottom:.5rem;color:#ef4444}p{color:#888;font-size:.875rem}</style></head>
<body><div class="c"><h1>Authentication Failed</h1><p>Please try again from your terminal.</p></div></body></html>"#;

pub async fn login(core_url: &str, provider_id: &str) -> Result<Tokens> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let callback = format!("http://127.0.0.1:{port}/callback");
    let authorize_url = format!(
        "{}/api/v1/auth/oidc/{}/authorize?redirect_uri={}",
        core_url.trim_end_matches('/'),
        urlencoding::encode(provider_id),
        urlencoding::encode(&callback),
    );

    println!("→ opening browser for OIDC login ({provider_id})");
    if webbrowser::open(&authorize_url).is_err() {
        println!("  could not open browser automatically — open this URL manually:");
        println!("  {authorize_url}");
    }

    tokio::time::timeout(Duration::from_secs(120), wait_for_callback(listener))
        .await
        .map_err(|_| anyhow!("OIDC login timed out (120s)"))?
}

async fn wait_for_callback(listener: TcpListener) -> Result<Tokens> {
    let (mut stream, _) = listener.accept().await?;
    drop(listener);

    let mut buf = vec![0u8; 8192];
    let mut total = 0;
    loop {
        let n = stream.read(&mut buf[total..]).await?;
        if n == 0 {
            break;
        }
        total += n;
        if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if total >= buf.len() {
            break;
        }
    }

    let request = String::from_utf8_lossy(&buf[..total]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| anyhow!("invalid HTTP request"))?;

    let query = path.split('?').nth(1).unwrap_or("");
    match parse_callback(query) {
        Ok(tokens) => {
            send_response(&mut stream, "200 OK", SUCCESS_HTML).await;
            let _ = stream.shutdown().await;
            Ok(tokens)
        }
        Err(e) => {
            send_response(&mut stream, "400 Bad Request", ERROR_HTML).await;
            let _ = stream.shutdown().await;
            Err(e)
        }
    }
}

pub(crate) fn parse_callback(query: &str) -> Result<Tokens> {
    let mut params: HashMap<String, String> = query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            Some((
                parts.next()?.to_string(),
                urlencoding::decode(parts.next()?).ok()?.to_string(),
            ))
        })
        .collect();
    match (params.remove("access_token"), params.remove("refresh_token")) {
        (Some(at), Some(rt)) => Ok(Tokens { access_token: at, refresh_token: rt }),
        _ => {
            let err = params.remove("error").unwrap_or_else(|| "unknown error".into());
            bail!("OIDC callback error: {err}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_callback;

    #[test]
    fn happy_path_decodes_both_tokens() {
        let t = parse_callback("access_token=abc&refresh_token=xyz").unwrap();
        assert_eq!(t.access_token, "abc");
        assert_eq!(t.refresh_token, "xyz");
    }

    #[test]
    fn url_decodes_values() {
        let t = parse_callback("access_token=a%20b&refresh_token=r%2Fz").unwrap();
        assert_eq!(t.access_token, "a b");
        assert_eq!(t.refresh_token, "r/z");
    }

    #[test]
    fn missing_refresh_is_error() {
        assert!(parse_callback("access_token=abc").is_err());
    }

    #[test]
    fn error_param_is_surfaced() {
        let e = parse_callback("error=access_denied").unwrap_err().to_string();
        assert!(e.contains("access_denied"), "got: {e}");
    }

    #[test]
    fn empty_query_is_error() {
        let e = parse_callback("").unwrap_err().to_string();
        assert!(e.contains("unknown error"));
    }
}

async fn send_response(stream: &mut TcpStream, status: &str, html: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len(),
    );
    let _ = stream.write_all(response.as_bytes()).await;
}
