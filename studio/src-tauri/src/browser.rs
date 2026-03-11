use std::sync::LazyLock;

use rootcx_browser::{BrowserSession, SnapshotMode};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tracing::{error, info, warn};

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);
static SESSION: LazyLock<Mutex<Option<BrowserSession>>> = LazyLock::new(|| Mutex::new(None));
static CORE_URL: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));

pub fn spawn_listener(core_url: String) {
    tokio::spawn(async move {
        *CORE_URL.lock().await = core_url;
        loop {
            if let Err(e) = listen_commands().await {
                warn!("browser command listener error: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

async fn listen_commands() -> Result<(), String> {
    let url = format!("{}/api/v1/browser/commands", CORE_URL.lock().await);
    let resp = HTTP.get(&url).send().await.map_err(|e| format!("connect: {e}"))?;
    if !resp.status().is_success() { return Err(format!("status {}", resp.status())); }
    info!("browser command listener connected");

    let mut buf = String::new();
    let mut body = resp;
    loop {
        match body.chunk().await {
            Ok(Some(chunk)) => {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                let mut consumed = 0;
                while let Some(pos) = buf[consumed..].find('\n') {
                    let end = consumed + pos;
                    let line = buf[consumed..end].trim_end_matches('\r');
                    if let Some(data) = line.strip_prefix("data:")
                        && let Ok(cmd) = serde_json::from_str::<Value>(data.trim_start())
                    {
                        tokio::spawn(execute_command(cmd));
                    }
                    consumed = end + 1;
                }
                if consumed > 0 { buf.drain(..consumed); }
            }
            Ok(None) => break,
            Err(e) => return Err(format!("chunk: {e}")),
        }
    }
    Ok(())
}

async fn execute_command(cmd: Value) {
    let id = cmd["id"].as_u64().unwrap_or(0);
    let action = cmd["action"].as_str().unwrap_or("");
    let params = &cmd["params"];
    let t0 = std::time::Instant::now();

    let result = match dispatch(action, params).await {
        Ok(val) => {
            info!(id, action, ms = t0.elapsed().as_millis() as u64, "browser: ok");
            val
        }
        Err(e) => {
            error!(id, action, ms = t0.elapsed().as_millis() as u64, "browser: {e}");
            json!({ "error": format!("{e} (took {}ms)", t0.elapsed().as_millis()) })
        }
    };

    let url = format!("{}/api/v1/browser/result/{id}", CORE_URL.lock().await);
    if let Err(e) = HTTP.post(&url).json(&result).send().await {
        error!(id, "browser result post failed: {e}");
    }
}

fn ok_url(s: &BrowserSession) -> Value {
    json!({ "ok": true, "url": s.page_url() })
}

fn parse_mode(params: &Value) -> SnapshotMode {
    match params["mode"].as_str() {
        Some("efficient") => SnapshotMode::Efficient,
        _ => SnapshotMode::Full,
    }
}

fn ref_id(params: &Value) -> Result<u32, String> {
    params["ref_id"].as_u64().map(|v| v as u32).ok_or_else(|| "missing ref_id".into())
}

async fn dispatch(action: &str, params: &Value) -> Result<Value, String> {
    let mut guard = SESSION.lock().await;
    if guard.is_none() {
        info!("launching browser...");
        *guard = Some(BrowserSession::launch().await.map_err(|e| format!("launch: {e}"))?);
        info!("browser launched");
    }
    let s = guard.as_mut().ok_or("session unavailable")?;

    match action {
        "navigate" => {
            let url = params["url"].as_str().ok_or("missing url")?;
            serde_json::to_value(s.navigate(url).await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "snapshot" => {
            serde_json::to_value(s.snapshot(parse_mode(params)).await.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "click" => {
            s.click(ref_id(params)?).await.map_err(|e| e.to_string())?;
            Ok(ok_url(s))
        }
        "type" => {
            let id = ref_id(params)?;
            let text = params["text"].as_str().ok_or("missing text")?;
            s.type_keys(id, text).await.map_err(|e| e.to_string())?;
            Ok(ok_url(s))
        }
        "scroll" => {
            let dir = params["direction"].as_str().unwrap_or("down");
            let amt = params["amount"].as_u64().unwrap_or(3) as u32;
            s.scroll(dir, amt).await.map_err(|e| e.to_string())?;
            Ok(ok_url(s))
        }
        "press_key" => {
            let key = params["key"].as_str().ok_or("missing key")?;
            s.press_key(key).await.map_err(|e| e.to_string())?;
            Ok(ok_url(s))
        }
        "select_option" => {
            let id = ref_id(params)?;
            let val = params["value"].as_str().ok_or("missing value")?;
            s.select_option(id, val).await.map_err(|e| e.to_string())?;
            Ok(ok_url(s))
        }
        "hover" => {
            s.hover(ref_id(params)?).await.map_err(|e| e.to_string())?;
            Ok(ok_url(s))
        }
        _ => Err(format!("unknown action: {action}")),
    }
}

pub async fn shutdown() {
    *SESSION.lock().await = None;
    info!("browser session closed");
}
