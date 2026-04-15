use anyhow::{Context, Result};

use crate::client_from_config;

pub async fn list(json: bool) -> Result<()> {
    let client = client_from_config().await?;
    let agents = client.list_all_agents().await.context("list agents failed")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }
    if agents.is_empty() {
        eprintln!("No agents deployed.");
        return Ok(());
    }
    let width = agents.iter()
        .filter_map(|a| a["app_id"].as_str().map(str::len))
        .max()
        .unwrap_or(0);
    for agent in agents {
        let app_id = agent["app_id"].as_str().unwrap_or("?");
        let name = agent["name"].as_str().unwrap_or("");
        println!("{app_id:width$}  {name}", width = width);
    }
    Ok(())
}

pub async fn sessions(app_id: &str, json: bool) -> Result<()> {
    let client = client_from_config().await?;
    let sessions = client.list_agent_sessions(app_id).await.context("list sessions failed")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }
    if sessions.is_empty() {
        eprintln!("No sessions for '{app_id}'.");
        return Ok(());
    }
    for s in sessions {
        let id = s["id"].as_str().unwrap_or("?");
        let title = s["title"].as_str().unwrap_or("");
        let updated = s["updated_at"].as_str().unwrap_or("");
        println!("{id}  {updated}  {title}");
    }
    Ok(())
}
