use anyhow::{Context, Result};
use std::time::{Duration, SystemTime};

const REPO_OWNER: &str = "RootCX";
const REPO_NAME: &str = "RootCX";
const BIN_NAME: &str = "rootcx";
const CHECK_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Upgrade the rootcx binary in place to the latest (or specified) GitHub release.
pub async fn run(version: Option<String>) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    tokio::task::spawn_blocking(move || upgrade_blocking(current, version))
        .await
        .context("upgrade task panicked")?
}

fn upgrade_blocking(current: String, version: Option<String>) -> Result<()> {
    let target = self_update::get_target();
    let identifier = format!("rootcx-{target}.tar.gz");

    let tag = match version {
        Some(ref v) => format!("cli-v{}", v.trim_start_matches('v').trim_start_matches("cli-v")),
        None => latest_cli_tag()?,
    };
    let resolved = tag.trim_start_matches("cli-v");

    if !self_update::version::bump_is_greater(&current, resolved).unwrap_or(false) && version.is_none() {
        println!("✓ rootcx {current} is already up to date");
        return Ok(());
    }

    self_update::backends::github::Update::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .identifier(&identifier)
        .current_version(&current)
        .target_version_tag(&tag)
        .show_download_progress(true)
        .no_confirm(true)
        .build().context("failed to configure updater")?
        .update().context("upgrade failed")?;
    println!("✓ upgraded rootcx {current} -> {resolved}");
    Ok(())
}

fn latest_cli_tag() -> Result<String> {
    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build().context("failed to build release list")?
        .fetch().context("failed to fetch releases")?;
    releases.iter()
        .find(|r| r.version.starts_with("cli-v"))
        .map(|r| r.version.clone())
        .context("no CLI release found")
}

/// Prints a hint if a newer release exists. Cached 24h in ~/.rootcx/last-update-check
/// to avoid hitting the GitHub API on every invocation. Silent on error.
pub async fn check_passive() {
    if std::env::var("ROOTCX_NO_UPDATE_CHECK").is_ok() {
        return;
    }
    let current = env!("CARGO_PKG_VERSION").to_string();
    let _ = tokio::task::spawn_blocking(move || check_passive_blocking(&current)).await;
}

fn check_passive_blocking(current: &str) -> Option<()> {
    let cache = cache_path()?;
    if cache_fresh(&cache) {
        return None;
    }
    // Touch cache first: even if the fetch fails, we don't retry for 24h.
    let _ = std::fs::write(&cache, "");

    let tag = latest_cli_tag().ok()?;
    let latest = tag.trim_start_matches("cli-v");
    if self_update::version::bump_is_greater(current, latest).unwrap_or(false) {
        eprintln!("\x1b[2m(rootcx {latest} available — run `rootcx upgrade`)\x1b[0m");
    }
    Some(())
}

fn cache_path() -> Option<std::path::PathBuf> {
    let dir = dirs::home_dir()?.join(".rootcx");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("last-update-check"))
}

fn cache_fresh(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map(|age| age < CHECK_TTL)
        .unwrap_or(false)
}
