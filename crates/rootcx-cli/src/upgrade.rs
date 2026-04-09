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

    let mut builder = self_update::backends::github::Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .identifier(&identifier)
        .current_version(&current)
        .show_download_progress(true)
        .no_confirm(true);

    if let Some(v) = version.as_deref() {
        let tag = format!("v{}", v.trim_start_matches('v'));
        builder.target_version_tag(&tag);
    }

    let updater = builder.build().context("failed to configure self_update")?;
    let status = updater.update().context("upgrade failed")?;

    if status.updated() {
        println!("✓ upgraded rootcx {} → {}", current, status.version());
    } else {
        println!("✓ rootcx {} is already up to date", current);
    }
    Ok(())
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

    let releases = self_update::backends::github::ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()
        .ok()?
        .fetch()
        .ok()?;
    let latest = releases.first()?.version.trim_start_matches('v');
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
