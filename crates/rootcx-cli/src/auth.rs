use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::config::{self, Config};
use crate::oidc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuthMode {
    #[serde(default)]
    providers: Vec<OidcProvider>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OidcProvider {
    id: String,
    display_name: String,
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

    let mode: AuthMode = http.get(format!("{base}/api/v1/auth/mode"))
        .send().await.context("could not discover SSO providers")?
        .error_for_status().context("Core rejected SSO discovery")?
        .json().await.context("invalid SSO provider response")?;

    let provider = match mode.providers.as_slice() {
        [] => bail!("no SSO provider configured. Configure ROOTCX_OIDC_ISSUER, ROOTCX_OIDC_CLIENT_ID and ROOTCX_OIDC_CLIENT_SECRET on Core"),
        [provider] => provider,
        providers => {
            let mut select = cliclack::select("Sign-in provider");
            for (index, provider) in providers.iter().enumerate() {
                select = select.item(index, &provider.display_name, "");
            }
            &providers[select.interact()?]
        }
    };
    println!("→ authenticating via {} (OIDC)", provider.display_name);
    let tokens = oidc::login(base, &provider.id).await?;
    let cfg = Config {
        url: base.to_string(),
        token: Some(tokens.access_token),
        refresh_token: Some(tokens.refresh_token),
    };

    config::save(&cfg)?;
    println!("✓ connected to {base} (authenticated)");
    Ok(())
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
