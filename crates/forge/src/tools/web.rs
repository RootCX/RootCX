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

pub async fn fetch(args: Value, _cwd: &Path) -> Result<String, String> {
    let url = args["url"].as_str().ok_or("missing url")?;
    let max_len = args["max_length"].as_u64().unwrap_or(30_000) as usize;

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
