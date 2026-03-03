use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use serde_json::Value;

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (compatible; RootCX/1.0)")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("http client")
});

fn is_private_ip(host: &str) -> bool {
    use std::net::IpAddr;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local()
                || v4.octets()[0] == 169 && v4.octets()[1] == 254,
            IpAddr::V6(v6) => v6.is_loopback(),
        };
    }
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
}

pub async fn fetch(args: Value, _cwd: &Path) -> Result<String, String> {
    let url = args["url"].as_str().ok_or("missing url")?;
    let max_len = args["max_length"].as_u64().unwrap_or(30_000) as usize;

    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("URL scheme '{}' not allowed; use http or https", parsed.scheme()));
    }
    if let Some(host) = parsed.host_str() {
        if is_private_ip(host) {
            return Err("requests to private/loopback addresses are blocked".into());
        }
    }

    let resp = CLIENT.get(url).send().await.map_err(|e| format!("fetch: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let is_html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("text/html") || ct.contains("xhtml"));

    let body = resp.bytes().await.map_err(|e| format!("body: {e}"))?;

    let text = if is_html {
        html2text::from_read(&body[..], 120).map_err(|e| format!("html: {e}"))?
    } else {
        String::from_utf8_lossy(&body).into_owned()
    };

    if text.len() <= max_len {
        return Ok(text);
    }
    // Safe truncation: find valid char boundary
    let mut end = max_len;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(format!("{}\n... [truncated]", &text[..end]))
}
