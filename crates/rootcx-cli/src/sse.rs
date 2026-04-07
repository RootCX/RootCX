use anyhow::Result;
use futures_util::StreamExt;
use rootcx_client::RuntimeClient;
use serde_json::{json, Value};
use std::io::Write;

pub async fn invoke(client: &RuntimeClient, app_id: &str, message: &str, session: Option<&str>) -> Result<()> {
    let base = client.base_url();
    let token = client.token();
    let url = format!("{base}/api/v1/apps/{app_id}/agent/invoke");
    let mut body = json!({ "message": message });
    if let Some(s) = session {
        body["session_id"] = json!(s);
    }
    let http = reqwest::Client::new();
    let mut req = http.post(&url).json(&body);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("invoke failed: {}", resp.status());
    }

    let mut current_event = String::new();
    let mut data_buf = String::new();
    let mut leftover = String::new();

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        leftover.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = leftover.find('\n') {
            let line = leftover[..pos].trim_end_matches('\r').to_string();
            leftover.drain(..pos + 1);
            if line.is_empty() {
                if !data_buf.is_empty() {
                    render_event(&current_event, &data_buf);
                    data_buf.clear();
                    current_event.clear();
                }
            } else if let Some(evt) = line.strip_prefix("event: ") {
                current_event.clear();
                current_event.push_str(evt);
            } else if let Some(data) = line.strip_prefix("data: ") {
                data_buf.push_str(data);
            }
        }
    }
    if !data_buf.is_empty() {
        render_event(&current_event, &data_buf);
    }
    println!();
    Ok(())
}

fn render_event(event: &str, data: &str) {
    let v = match serde_json::from_str::<Value>(data) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[parse error: {e}]");
            return;
        }
    };
    match event {
        "chunk" | "sub_agent_chunk" => {
            if let Some(delta) = v["delta"].as_str() {
                print!("{delta}");
                let _ = std::io::stdout().flush();
            }
        }
        "tool_call_started" => {
            let name = v["tool_name"].as_str().unwrap_or("?");
            let input = &v["input"];
            if matches!(input, Value::Null) || input.as_object().is_some_and(|m| m.is_empty()) {
                eprintln!("\n→ {name}()");
            } else {
                eprintln!("\n→ {name}({})", input);
            }
        }
        "tool_call_completed" => {
            if let Some(err) = v["error"].as_str() {
                eprintln!("  ✗ {err}");
            }
        }
        "error" => {
            if let Some(err) = v["error"].as_str() {
                eprintln!("\n✗ {err}");
            }
        }
        "done" => {
            if let Some(sid) = v["session_id"].as_str() {
                eprintln!("\n[session: {sid}]");
            }
        }
        "approval_required" => {
            let tool = v["tool_name"].as_str().unwrap_or("?");
            let reason = v["reason"].as_str().unwrap_or("");
            eprintln!("\n⚠ approval required: {tool} — {reason}");
        }
        _ => {}
    }
}
