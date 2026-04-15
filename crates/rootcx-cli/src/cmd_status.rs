use anyhow::Result;
use rootcx_client::RuntimeClient;
use rootcx_types::AppManifest;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::{auth, config};

#[derive(Serialize)]
struct StatusReport {
    core: Option<CoreReport>,
    project: Option<ProjectReport>,
}

#[derive(Serialize)]
struct CoreReport {
    url: String,
    reachable: bool,
    authenticated: bool,
    user: Option<JsonValue>,
    runtime_version: Option<String>,
}

#[derive(Serialize)]
struct ProjectReport {
    app_id: String,
    manifest_path: String,
}

pub async fn run(json: bool) -> Result<()> {
    let report = collect().await;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    print_pretty(&report);
    Ok(())
}

async fn collect() -> StatusReport {
    StatusReport {
        core: collect_core().await,
        project: collect_project(),
    }
}

async fn collect_core() -> Option<CoreReport> {
    let mut cfg = config::load().ok()?;
    let url = cfg.url.clone();
    let client = RuntimeClient::new(&url);
    let reachable = client.is_available().await;
    if !reachable {
        return Some(CoreReport { url, reachable: false, authenticated: false, user: None, runtime_version: None });
    }
    let _ = auth::ensure_valid_token(&mut cfg).await;
    if let Some(t) = cfg.token.take() {
        client.set_token(Some(t));
    }
    let (me_result, status_result) = tokio::join!(client.me(), client.status());
    let (authenticated, user) = match me_result {
        Ok(u) => (true, Some(u)),
        Err(_) => (false, None),
    };
    let runtime_version = status_result.ok().map(|s| s.runtime.version);
    Some(CoreReport { url, reachable: true, authenticated, user, runtime_version })
}

fn collect_project() -> Option<ProjectReport> {
    let cwd = std::env::current_dir().ok()?;
    let manifest_path = cwd.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).ok()?;
    let manifest: AppManifest = serde_json::from_str(&raw).ok()?;
    Some(ProjectReport {
        app_id: manifest.app_id,
        manifest_path: manifest_path.display().to_string(),
    })
}

fn print_pretty(r: &StatusReport) {
    match &r.core {
        None => {
            println!("Core:     not connected");
            println!("          → run `rootcx auth login <url>` or `rootcx init`");
        }
        Some(c) if !c.reachable => {
            println!("Core:     {} (unreachable)", c.url);
        }
        Some(c) => {
            let version = c.runtime_version.as_deref().unwrap_or("unknown");
            println!("Core:     {} (v{version})", c.url);
            match (c.authenticated, &c.user) {
                (true, Some(u)) => {
                    let email = u["email"].as_str().unwrap_or("?");
                    println!("User:     {email}");
                }
                _ => {
                    println!("User:     not signed in");
                    println!("          → run `rootcx auth login`");
                }
            }
        }
    }
    match &r.project {
        Some(p) => println!("Project:  {} ({})", p.app_id, p.manifest_path),
        None => println!("Project:  no manifest.json in current directory"),
    }
}
