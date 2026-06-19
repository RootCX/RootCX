use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::io::Write;

use crate::client_from_config;

pub async fn list(json_flag: bool) -> Result<()> {
    let client = client_from_config().await?;
    let base = client.base_url();
    let token = client.token();
    let http = reqwest::Client::new();
    let mut req = http.get(format!("{base}/api/v1/workflows"));
    if let Some(t) = token { req = req.bearer_auth(t); }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("list workflows failed: {}", resp.status());
    }
    let workflows: Vec<Value> = resp.json().await?;
    if json_flag {
        println!("{}", serde_json::to_string_pretty(&workflows)?);
        return Ok(());
    }
    if workflows.is_empty() {
        eprintln!("No workflows.");
        return Ok(());
    }
    let w_id = workflows.iter().map(|w| w["id"].as_str().map_or(2, str::len)).max().unwrap_or(2);
    let w_name = workflows.iter().map(|w| w["name"].as_str().map_or(4, str::len)).max().unwrap_or(4);
    println!("{:<w_id$}  {:<w_name$}  {:>7}  {:>7}", "ID", "NAME", "ENABLED", "VERSION");
    for w in workflows {
        let id = w["id"].as_str().unwrap_or("?");
        let name = w["name"].as_str().unwrap_or("");
        let enabled = if w["enabled"].as_bool().unwrap_or(false) { "yes" } else { "no" };
        let version = w["version"].as_i64().unwrap_or(0);
        println!("{:<w_id$}  {:<w_name$}  {:>7}  {:>7}", id, name, enabled, version);
    }
    Ok(())
}

pub async fn describe(id: &str) -> Result<()> {
    let wf = get_workflow(id).await?;
    println!("{}", serde_json::to_string_pretty(&wf)?);
    Ok(())
}

pub async fn run(id: &str) -> Result<()> {
    let client = client_from_config().await?;
    let base = client.base_url();
    let token = client.token();
    let http = reqwest::Client::new();

    // Trigger the run
    let mut req = http.post(format!("{base}/api/v1/workflows/{id}/run"));
    if let Some(ref t) = token { req = req.bearer_auth(t); }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("run workflow failed: {}", resp.status());
    }
    let body: Value = resp.json().await?;
    let exec_id = body["executionId"].as_str()
        .context("missing executionId in response")?;
    eprintln!("execution {exec_id} queued");

    // Stream SSE progress
    let mut req = http.get(format!("{base}/api/v1/workflows/{id}/executions/{exec_id}/stream"));
    if let Some(ref t) = token { req = req.bearer_auth(t); }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("stream execution failed: {}", resp.status());
    }

    let mut current_event = String::new();
    let mut data_buf = String::new();
    let mut leftover = String::new();
    let mut final_status = String::new();

    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        leftover.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = leftover.find('\n') {
            let line = leftover[..pos].trim_end_matches('\r').to_string();
            leftover.drain(..pos + 1);
            if line.is_empty() {
                if !data_buf.is_empty() {
                    final_status = render_workflow_event(&current_event, &data_buf);
                    data_buf.clear();
                    current_event.clear();
                }
            } else if let Some(evt) = line.strip_prefix("event: ") {
                current_event = evt.to_string();
            } else if let Some(data) = line.strip_prefix("data: ") {
                data_buf.push_str(data);
            }
        }
    }
    if !data_buf.is_empty() {
        final_status = render_workflow_event(&current_event, &data_buf);
    }

    if final_status == "failed" {
        std::process::exit(1);
    }
    Ok(())
}

fn render_workflow_event(event: &str, data: &str) -> String {
    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    match event {
        "node" => {
            let node_id = v["nodeId"].as_str().unwrap_or("?");
            let status = v["status"].as_str().unwrap_or("?");
            let icon = match status {
                "succeeded" => "\u{2713}",
                "failed" => "\u{2717}",
                "running" => "\u{2192}",
                _ => " ",
            };
            eprintln!("{icon} {node_id} ({status})");
            let _ = std::io::stderr().flush();
            String::new()
        }
        "done" => {
            let status = v["status"].as_str().unwrap_or("?");
            if let Some(err) = v["error"].as_str() {
                eprintln!("done: {status} - {err}");
            } else {
                eprintln!("done: {status}");
            }
            status.to_string()
        }
        _ => String::new(),
    }
}

pub async fn enable(id: &str) -> Result<()> {
    set_enabled(id, true).await
}

pub async fn disable(id: &str) -> Result<()> {
    set_enabled(id, false).await
}

async fn set_enabled(id: &str, enabled: bool) -> Result<()> {
    let client = client_from_config().await?;
    let base = client.base_url();
    let token = client.token();
    let http = reqwest::Client::new();
    let mut req = http.put(format!("{base}/api/v1/workflows/{id}"))
        .json(&json!({ "enabled": enabled }));
    if let Some(t) = token { req = req.bearer_auth(t); }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("update workflow failed: {}", resp.status());
    }
    let state = if enabled { "enabled" } else { "disabled" };
    println!("\u{2713} workflow {id} {state}");
    Ok(())
}

pub async fn rm(id: &str, yes: bool) -> Result<()> {
    if !yes {
        cliclack::set_theme(crate::theme::RootcxTheme);
        let confirmed = cliclack::confirm(format!("Delete workflow '{id}'? This cannot be undone."))
            .initial_value(false)
            .interact()?;
        if !confirmed {
            println!("Cancelled");
            return Ok(());
        }
    }
    let client = client_from_config().await?;
    let base = client.base_url();
    let token = client.token();
    let http = reqwest::Client::new();
    let mut req = http.delete(format!("{base}/api/v1/workflows/{id}"));
    if let Some(t) = token { req = req.bearer_auth(t); }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("delete workflow failed: {}", resp.status());
    }
    println!("\u{2713} deleted {id}");
    Ok(())
}

pub async fn export(id: &str) -> Result<()> {
    let wf = get_workflow(id).await?;
    let graph = &wf["graph"];
    println!("{}", serde_json::to_string_pretty(graph)?);
    Ok(())
}

pub async fn import(file: &str) -> Result<()> {
    let content = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read '{file}'"))?;
    let graph: Value = serde_json::from_str(&content)
        .context("invalid JSON in file")?;

    let name = std::path::Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("imported");

    let client = client_from_config().await?;
    let base = client.base_url();
    let token = client.token();
    let http = reqwest::Client::new();
    let mut req = http.post(format!("{base}/api/v1/workflows"))
        .json(&json!({ "name": name, "graph": graph }));
    if let Some(t) = token { req = req.bearer_auth(t); }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or_default();
        let msg = body["error"].as_str().unwrap_or("unknown error");
        anyhow::bail!("create workflow failed ({status}): {msg}");
    }
    let body: Value = resp.json().await?;
    let id = body["id"].as_str().unwrap_or("?");
    println!("\u{2713} created workflow {id}");
    Ok(())
}

async fn get_workflow(id: &str) -> Result<Value> {
    let client = client_from_config().await?;
    let base = client.base_url();
    let token = client.token();
    let http = reqwest::Client::new();
    let mut req = http.get(format!("{base}/api/v1/workflows/{id}"));
    if let Some(t) = token { req = req.bearer_auth(t); }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("get workflow failed: {}", resp.status());
    }
    Ok(resp.json().await?)
}
