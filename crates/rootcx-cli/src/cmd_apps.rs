use anyhow::{Context, Result};

use crate::{client_from_config, theme};

pub async fn list(json: bool) -> Result<()> {
    let client = client_from_config().await?;
    let apps = client.list_apps().await.context("list apps failed")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&apps)?);
        return Ok(());
    }
    if apps.is_empty() {
        eprintln!("No apps installed.");
        return Ok(());
    }
    let width = apps.iter().map(|a| a.id.len()).max().unwrap_or(0);
    for app in apps {
        println!("{:width$}  {}", app.id, app.name, width = width);
    }
    Ok(())
}

pub async fn describe(app_id: &str, json: bool) -> Result<()> {
    let client = client_from_config().await?;
    let app = client.get_app(app_id).await.context("get app failed")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&app)?);
        return Ok(());
    }
    let name = app["name"].as_str().unwrap_or("");
    let version = app["version"].as_str().unwrap_or("");
    println!("{app_id}  {name}  v{version}\n");
    if let Some(entities) = app["dataContract"].as_array() {
        for entity in entities {
            let ename = entity["entityName"].as_str().unwrap_or("?");
            println!("  {ename}");
            if let Some(fields) = entity["fields"].as_array() {
                for field in fields {
                    let fname = field["name"].as_str().unwrap_or("?");
                    let ftype = field["type"].as_str().unwrap_or("?");
                    let req = if field["required"].as_bool().unwrap_or(false) {
                        " *"
                    } else {
                        ""
                    };
                    println!("    {fname}: {ftype}{req}");
                }
            }
        }
    }
    Ok(())
}

pub async fn rm(app_id: &str, yes: bool) -> Result<()> {
    let client = client_from_config().await?;
    remove(&client, app_id, yes).await
}

async fn remove(client: &rootcx_client::RuntimeClient, app_id: &str, yes: bool) -> Result<()> {
    let app = client
        .list_apps()
        .await?
        .into_iter()
        .find(|app| app.id == app_id)
        .context("installed app not found")?;
    if app.app_type != rootcx_types::AppType::App {
        if !confirm_removal(&app.name, yes)? {
            return Ok(());
        }
        match app.app_type {
            rootcx_types::AppType::Agent => {
                #[allow(deprecated)]
                client
                    .uninstall_app(app_id)
                    .await
                    .context("agent uninstall failed")?;
            }
            rootcx_types::AppType::Integration => client
                .uninstall_integration(app_id)
                .await
                .context("integration uninstall failed")?,
            rootcx_types::AppType::App => unreachable!(),
        }
        println!("✓ uninstalled {app_id}");
        return Ok(());
    }
    let operation = client
        .prepare_app_deletion(app_id)
        .await
        .context("prepare deletion failed")?;
    let id = operation["id"]
        .as_str()
        .context("Core returned no deletion operation")?;
    let name = operation["applicationName"].as_str().unwrap_or(app_id);
    if !confirm_removal(name, yes)? {
        return Ok(());
    }
    client.confirm_app_deletion(id).await.with_context(|| {
        format!("confirmation response failed; inspect /api/v1/app-deletions/{id} before retrying")
    })?;
    for _ in 0..30 {
        let operation = client.app_deletion(id).await?;
        if operation["state"] == "succeeded" {
            println!("✓ deleted {app_id}");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    println!(
        "Deletion {id} is still in progress. Core will continue automatically; inspect /api/v1/app-deletions/{id}."
    );
    Ok(())
}

fn confirm_removal(name: &str, yes: bool) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    cliclack::set_theme(theme::RootcxTheme);
    let confirmed = cliclack::confirm(format!(
        "Delete '{name}', its data, files and change history for everyone? This cannot be undone."
    ))
    .initial_value(false)
    .interact()?;
    if !confirmed {
        println!("Cancelled");
    }
    Ok(confirmed)
}

#[cfg(test)]
mod tests;
