use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::config::{self, Config};
use crate::oidc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthMode {
    setup_required: bool,
    password_login_enabled: bool,
    #[serde(default)]
    providers: Vec<OidcProvider>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OidcProvider {
    id: String,
    display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponse {
    access_token: String,
    refresh_token: String,
}

pub async fn connect(url: &str, token: Option<String>) -> Result<()> {
    let cfg = config::merge_config(config::load().ok(), url.to_string(), token);
    config::save(&cfg)?;

    let http = reqwest::Client::new();
    let base = &cfg.url;

    if !http.get(format!("{base}/health")).send().await.is_ok_and(|r| r.status().is_success()) {
        eprintln!("⚠ config saved but Core is not reachable at {base}");
        return Ok(());
    }

    if let Some(ref t) = cfg.token {
        if http.get(format!("{base}/api/v1/status")).bearer_auth(t).send().await.is_ok_and(|r| r.status().is_success()) {
            println!("✓ connected to {base} (authenticated)");
            return Ok(());
        }
    }

    let mode_resp = http.get(format!("{base}/api/v1/auth/mode")).send().await;
    let mode: Option<AuthMode> = match mode_resp {
        Ok(r) if r.status().is_success() => r.json().await.ok(),
        _ => None,
    };

    let Some(mode) = mode else {
        println!("✓ connected to {base}");
        return Ok(());
    };

    let cfg = if mode.setup_required {
        println!("→ first-time setup — create admin account");
        let (email, password) = prompt_credentials()?;
        register(&http, base, &email, &password).await?;
        println!("✓ registered {email} (admin)");
        password_login(&http, base, &email, &password).await?
    } else if mode.password_login_enabled && mode.providers.is_empty() {
        let (email, password) = prompt_credentials()?;
        password_login(&http, base, &email, &password).await?
    } else if !mode.providers.is_empty() {
        let provider = &mode.providers[0];
        println!("→ authenticating via {} (OIDC)", provider.display_name);
        oidc_login(base, &provider.id).await?
    } else {
        bail!("auth required but no login method available");
    };

    config::save(&cfg)?;
    println!("✓ connected to {base} (authenticated)");
    Ok(())
}

async fn register(http: &reqwest::Client, base: &str, email: &str, password: &str) -> Result<()> {
    let resp = http.post(format!("{base}/api/v1/auth/register"))
        .json(&json!({ "email": email, "password": password }))
        .send().await
        .context("register request failed")?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("register failed: {body}");
    }
    Ok(())
}

async fn password_login(http: &reqwest::Client, base: &str, email: &str, password: &str) -> Result<Config> {
    let resp = http.post(format!("{base}/api/v1/auth/login"))
        .json(&json!({ "email": email, "password": password }))
        .send().await
        .context("login request failed")?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        bail!("login failed: {body}");
    }
    let login: LoginResponse = resp.json().await?;
    Ok(Config { url: base.to_string(), token: Some(login.access_token), refresh_token: Some(login.refresh_token) })
}

async fn oidc_login(base: &str, provider_id: &str) -> Result<Config> {
    let tokens = oidc::login(base, provider_id).await?;
    Ok(Config { url: base.to_string(), token: Some(tokens.access_token), refresh_token: Some(tokens.refresh_token) })
}

pub async fn ensure_valid_token(cfg: &mut Config) -> Result<()> {
    let Some(ref rt) = cfg.refresh_token else { return Ok(()) };
    let http = reqwest::Client::new();
    let resp = http.post(format!("{}/api/v1/auth/refresh", cfg.url))
        .json(&json!({ "refreshToken": rt }))
        .send().await
        .context("refresh request failed")?;
    let new_token = if resp.status().is_success() {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct R { access_token: String }
        Some(resp.json::<R>().await?.access_token)
    } else {
        None
    };
    let ok = apply_refresh(cfg, new_token);
    config::save(cfg)?;
    if !ok {
        bail!("session expired. Run `rootcx auth login {}` to re-authenticate", cfg.url);
    }
    Ok(())
}

fn apply_refresh(cfg: &mut Config, new_access_token: Option<String>) -> bool {
    match new_access_token {
        Some(token) => {
            cfg.token = Some(token);
            true
        }
        None => {
            cfg.token = None;
            cfg.refresh_token = None;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(token: Option<&str>, refresh: Option<&str>) -> Config {
        Config {
            url: "http://core".into(),
            token: token.map(Into::into),
            refresh_token: refresh.map(Into::into),
        }
    }

    #[test]
    fn refresh_success_updates_token_preserves_refresh() {
        let mut c = cfg(Some("old"), Some("rt"));
        assert!(apply_refresh(&mut c, Some("new".into())));
        assert_eq!(c.token.as_deref(), Some("new"));
        assert_eq!(c.refresh_token.as_deref(), Some("rt"));
    }

    #[test]
    fn refresh_failure_clears_both_tokens() {
        let mut c = cfg(Some("old"), Some("rt"));
        assert!(!apply_refresh(&mut c, None));
        assert!(c.token.is_none());
        assert!(c.refresh_token.is_none());
    }
}

fn prompt_credentials() -> Result<(String, String)> {
    eprint!("  email: ");
    std::io::Write::flush(&mut std::io::stderr())?;
    let mut email = String::new();
    std::io::stdin().read_line(&mut email)?;
    let email = email.trim().to_string();
    if email.is_empty() {
        bail!("email required");
    }
    let password = rpassword::prompt_password("  password: ")?;
    if password.is_empty() {
        bail!("password required");
    }
    Ok((email, password))
}
