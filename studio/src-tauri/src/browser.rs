use std::sync::LazyLock;

use rootcx_browser::BrowserSession;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::state::DAEMON_URL;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);
static SESSION: LazyLock<Mutex<Option<BrowserSession>>> = LazyLock::new(|| Mutex::new(None));

pub fn spawn_listener() {
    tokio::spawn(async {
        loop {
            if let Err(e) = listen_commands().await {
                warn!("browser command listener error: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}

async fn listen_commands() -> Result<(), String> {
    let url = format!("{DAEMON_URL}/api/v1/browser/commands");
    let resp = HTTP.get(&url).send().await.map_err(|e| format!("connect: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }

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
                if consumed > 0 {
                    buf.drain(..consumed);
                }
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

    let result = match dispatch(action, params).await {
        Ok(val) => val,
        Err(e) => {
            error!(id, action, "browser command failed: {e}");
            serde_json::json!({ "error": e })
        }
    };

    let url = format!("{DAEMON_URL}/api/v1/browser/result/{id}");
    if let Err(e) = HTTP.post(&url).json(&result).send().await {
        error!(id, "failed to post browser result: {e}");
    }
}

async fn ensure_session() -> Result<(), String> {
    let mut session = SESSION.lock().await;
    if session.is_none() {
        info!("launching browser...");
        *session = Some(
            BrowserSession::launch()
                .await
                .map_err(|e| format!("browser launch failed: {e}"))?,
        );
        info!("browser launched");
    }
    Ok(())
}

async fn dispatch(action: &str, params: &Value) -> Result<Value, String> {
    ensure_session().await?;
    let mut session = SESSION.lock().await;
    let s = session.as_mut().unwrap();

    match action {
        "navigate" => {
            let url = params["url"].as_str().ok_or("missing url")?;
            let snap = s.navigate(url).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(snap).unwrap())
        }
        "snapshot" => {
            let snap = s.snapshot().await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(snap).unwrap())
        }
        "click" => {
            let ref_id = params["ref_id"].as_u64().ok_or("missing ref_id")? as u32;
            let snap = s.click(ref_id).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(snap).unwrap())
        }
        "type" => {
            let ref_id = params["ref_id"].as_u64().ok_or("missing ref_id")? as u32;
            let text = params["text"].as_str().ok_or("missing text")?;
            let snap = s.type_keys(ref_id, text).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(snap).unwrap())
        }
        "scroll" => {
            let direction = params["direction"].as_str().unwrap_or("down");
            let amount = params["amount"].as_u64().unwrap_or(3) as u32;
            let snap = s.scroll(direction, amount).await.map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(snap).unwrap())
        }
        _ => Err(format!("unknown action: {action}")),
    }
}

pub async fn shutdown() {
    *SESSION.lock().await = None;
    info!("browser session closed");
}
