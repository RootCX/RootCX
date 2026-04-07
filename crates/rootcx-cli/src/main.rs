use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rootcx_client::RuntimeClient;
use rootcx_types::AppManifest;
use std::path::Path;

mod archive;
mod auth;
mod config;
mod deploy;
mod oidc;
mod scaffold;
mod sse;
#[cfg(test)]
mod testutil;

#[derive(Parser)]
#[command(name = "rootcx", version, about = "RootCX CLI — code, deploy and manage RootCX apps")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Connect to a Core — saves URL, authenticates if needed
    Connect {
        url: String,
        #[arg(long)]
        token: Option<String>,
    },
    /// Clear stored tokens for the current workspace
    Logout,
    /// Show connected Core status
    Status,
    /// Scaffold a new app or agent
    New {
        kind: scaffold::ProjectKind,
        name: String,
    },
    /// Deploy current project to the connected Core
    Deploy,
    /// List installed apps on the Core
    Apps,
    /// Uninstall an app
    Uninstall { app_id: String },
    /// Invoke an agent (streams SSE)
    Invoke {
        app_id: String,
        message: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Print the path to bundled skills (for plugin use)
    SkillsPath,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Connect { url, token } => auth::connect(&url, token).await,
        Cmd::Logout => logout(),
        Cmd::Status => status().await,
        Cmd::New { kind, name } => scaffold::run(kind, &name, &std::env::current_dir()?),
        Cmd::Deploy => run_deploy().await,
        Cmd::Apps => apps().await,
        Cmd::Uninstall { app_id } => uninstall(&app_id).await,
        Cmd::Invoke { app_id, message, session } => {
            sse::invoke(&client_from_config()?, &app_id, &message, session.as_deref()).await
        }
        Cmd::SkillsPath => {
            println!("{}", config::skills_dir()?.display());
            Ok(())
        }
    }
}

fn client_from_config() -> Result<RuntimeClient> {
    let cfg = config::load().context("no .rootcx/config.json — run `rootcx connect <url>` first")?;
    let client = RuntimeClient::new(&cfg.url);
    if let Some(t) = cfg.token {
        client.set_token(Some(t));
    }
    Ok(client)
}

fn logout() -> Result<()> {
    let mut cfg = config::load().context("no .rootcx/config.json")?;
    cfg.token = None;
    cfg.refresh_token = None;
    config::save(&cfg)?;
    println!("✓ tokens cleared");
    Ok(())
}

async fn status() -> Result<()> {
    let client = client_from_config()?;
    let s = client.status().await.context("status request failed")?;
    println!("{}", serde_json::to_string_pretty(&s)?);
    Ok(())
}

async fn apps() -> Result<()> {
    let client = client_from_config()?;
    let list = client.list_apps().await.context("list_apps failed")?;
    for app in list {
        println!("{}  {}", app.id, app.name);
    }
    Ok(())
}

async fn uninstall(app_id: &str) -> Result<()> {
    let client = client_from_config()?;
    client.uninstall_app(app_id).await.context("uninstall failed")?;
    println!("✓ uninstalled {app_id}");
    Ok(())
}

async fn run_deploy() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let manifest_path = cwd.join("manifest.json");
    if !manifest_path.exists() {
        bail!("no manifest.json in {}", cwd.display());
    }
    let manifest: AppManifest = serde_json::from_str(&std::fs::read_to_string(&manifest_path)?)
        .context("invalid manifest.json")?;
    let app_id = manifest.app_id.clone();
    let plan = deploy::plan_deploy(&cwd);

    let client = client_from_config()?;

    println!("→ installing manifest ({})", app_id);
    client.install_app(&manifest).await.context("install_app failed")?;

    if plan.backend {
        println!("→ packaging backend/");
        let tar = archive::pack_dir(&cwd, Path::new("backend"))?;
        println!("→ uploading backend ({} bytes)", tar.len());
        client.deploy_app(&app_id, tar).await.context("deploy_app failed")?;
    }

    if plan.frontend {
        println!("→ packaging dist/");
        let tar = archive::pack_dir(&cwd, Path::new("dist"))?;
        println!("→ uploading frontend ({} bytes)", tar.len());
        client.deploy_frontend(&app_id, tar).await.context("deploy_frontend failed")?;
    } else if plan.warn_missing_dist {
        eprintln!("ℹ skipping frontend: no dist/ (run your build first)");
    }

    if plan.backend {
        println!("→ starting worker");
        match client.start_worker(&app_id).await {
            Ok(msg) => println!("  {msg}"),
            Err(e) => eprintln!("  ⚠ worker start: {e}"),
        }
    }

    println!("✓ deployed {app_id}");
    Ok(())
}
