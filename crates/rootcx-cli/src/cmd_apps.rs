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

pub async fn rm(app_id: &str, yes: bool) -> Result<()> {
    if !yes {
        cliclack::set_theme(theme::RootcxTheme);
        let confirmed = cliclack::confirm(format!("Uninstall '{app_id}'? This cannot be undone."))
            .initial_value(false)
            .interact()?;
        if !confirmed {
            println!("Cancelled");
            return Ok(());
        }
    }
    let client = client_from_config().await?;
    client.uninstall_app(app_id).await.context("uninstall failed")?;
    println!("✓ uninstalled {app_id}");
    Ok(())
}
