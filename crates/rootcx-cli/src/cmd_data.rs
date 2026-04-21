use anyhow::{Context, Result};
use serde_json::Value as JsonValue;

use crate::client_from_config;

pub async fn list(app_id: &str, entity: &str, limit: Option<u32>, offset: Option<u32>, sort: Option<&str>, order: Option<&str>) -> Result<()> {
    let client = client_from_config().await?;
    let mut body = serde_json::json!({ "limit": limit.unwrap_or(100) });
    if let Some(o) = offset { body["offset"] = o.into(); }
    if let Some(s) = sort { body["orderBy"] = s.into(); }
    if let Some(o) = order { body["order"] = o.into(); }
    let result = client.query_records(app_id, entity, &body).await.context("list records failed")?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub async fn query(app_id: &str, entity: &str, body: &str) -> Result<()> {
    let client = client_from_config().await?;
    let parsed: JsonValue = serde_json::from_str(body).context("invalid JSON body")?;
    let result = client.query_records(app_id, entity, &parsed).await.context("query records failed")?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub async fn get(app_id: &str, entity: &str, id: &str) -> Result<()> {
    let client = client_from_config().await?;
    let record = client.get_record(app_id, entity, id).await.context("get record failed")?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

pub async fn create(app_id: &str, entity: &str, body: &str) -> Result<()> {
    let client = client_from_config().await?;
    let data: JsonValue = serde_json::from_str(body).context("invalid JSON body")?;
    let record = client.create_record(app_id, entity, &data).await.context("create record failed")?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

pub async fn update(app_id: &str, entity: &str, id: &str, body: &str) -> Result<()> {
    let client = client_from_config().await?;
    let data: JsonValue = serde_json::from_str(body).context("invalid JSON body")?;
    let record = client.update_record(app_id, entity, id, &data).await.context("update record failed")?;
    println!("{}", serde_json::to_string_pretty(&record)?);
    Ok(())
}

pub async fn delete(app_id: &str, entity: &str, id: &str, yes: bool) -> Result<()> {
    if !yes {
        cliclack::set_theme(crate::theme::RootcxTheme);
        let confirmed = cliclack::confirm(format!("Delete record '{id}' from '{entity}'?"))
            .initial_value(false)
            .interact()?;
        if !confirmed {
            println!("Cancelled");
            return Ok(());
        }
    }
    let client = client_from_config().await?;
    client.delete_record(app_id, entity, id).await.context("delete record failed")?;
    println!("Deleted {id}");
    Ok(())
}
