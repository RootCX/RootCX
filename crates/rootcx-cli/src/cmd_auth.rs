use anyhow::{Context, Result};

use crate::{auth, config};

pub async fn login(url: Option<&str>, token: Option<String>) -> Result<()> {
    let url = resolve_login_url(url, config::load().ok())?;
    auth::connect(&url, token).await
}

fn resolve_login_url(arg: Option<&str>, saved: Option<config::Config>) -> Result<String> {
    if let Some(u) = arg {
        return Ok(u.to_string());
    }
    saved.map(|c| c.url).context("no saved Core. Run `rootcx auth login <url>`")
}

pub fn logout() -> Result<()> {
    let Ok(mut cfg) = config::load() else { return Ok(()) };
    if cfg.token.is_none() && cfg.refresh_token.is_none() {
        return Ok(());
    }
    cfg.token = None;
    cfg.refresh_token = None;
    config::save(&cfg)?;
    println!("✓ signed out");
    Ok(())
}

pub async fn whoami(json: bool) -> Result<()> {
    let client = crate::client_from_config().await?;
    let user = client.me().await.context("whoami failed")?;
    if json {
        println!("{}", serde_json::to_string_pretty(&user)?);
        return Ok(());
    }
    let email = user["email"].as_str().unwrap_or("?");
    let id = user["id"].as_str().unwrap_or("?");
    match user["displayName"].as_str().filter(|s| !s.is_empty()) {
        Some(name) => println!("{email} ({name}) [{id}]"),
        None => println!("{email} [{id}]"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static HOME_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = HOME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::TempDir::new().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()); }
        (tmp, guard)
    }

    #[test]
    fn logout_when_not_configured_is_silent() {
        let (_tmp, _g) = with_temp_home();
        assert!(logout().is_ok());
    }

    #[test]
    fn logout_when_already_logged_out_is_silent() {
        let (_tmp, _g) = with_temp_home();
        config::save(&config::Config {
            url: "http://core".into(),
            token: None,
            refresh_token: None,
        }).unwrap();
        assert!(logout().is_ok());
    }

    fn cfg(url: &str) -> config::Config {
        config::Config { url: url.into(), token: None, refresh_token: None }
    }

    #[test]
    fn resolve_url_prefers_arg_over_saved() {
        let got = resolve_login_url(Some("http://arg"), Some(cfg("http://saved"))).unwrap();
        assert_eq!(got, "http://arg");
    }

    #[test]
    fn resolve_url_falls_back_to_saved_when_no_arg() {
        let got = resolve_login_url(None, Some(cfg("http://saved"))).unwrap();
        assert_eq!(got, "http://saved");
    }

    #[test]
    fn resolve_url_errors_with_hint_when_neither() {
        let err = resolve_login_url(None, None).unwrap_err().to_string();
        assert!(err.contains("no saved Core"), "got: {err}");
        assert!(err.contains("rootcx auth login"), "should suggest the command: {err}");
    }

    #[test]
    fn logout_clears_tokens_preserves_url() {
        let (_tmp, _g) = with_temp_home();
        config::save(&config::Config {
            url: "http://core".into(),
            token: Some("t".into()),
            refresh_token: Some("r".into()),
        }).unwrap();
        logout().unwrap();
        let cfg = config::load().unwrap();
        assert_eq!(cfg.url, "http://core");
        assert!(cfg.token.is_none());
        assert!(cfg.refresh_token.is_none());
    }
}
