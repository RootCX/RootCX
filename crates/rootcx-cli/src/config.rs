use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

fn config_path() -> PathBuf {
    std::env::current_dir().unwrap_or_default().join(".rootcx").join("config.json")
}

pub fn load() -> Result<Config> {
    let path = config_path();
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_str(&raw)?)
}

pub fn save(cfg: &Config) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(cfg)?)?;
    Ok(())
}

/// Merge a new `connect` invocation with any existing config.
/// If the URL is unchanged and no new token is provided, prior tokens are preserved.
/// Otherwise (URL changed OR explicit token), tokens are replaced (refresh cleared).
pub fn merge_config(existing: Option<Config>, url: String, token: Option<String>) -> Config {
    let url = url.trim_end_matches('/').to_string();
    match existing {
        Some(c) if c.url == url && token.is_none() => {
            Config { url, token: c.token, refresh_token: c.refresh_token }
        }
        _ => Config { url, token, refresh_token: None },
    }
}

/// Resolve the bundled skills directory — env override > repo-relative > user config.
pub fn skills_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let home = dirs::home_dir().context("no home dir")?;
    Ok(resolve_skills_dir(std::env::var("ROOTCX_SKILLS_DIR").ok(), Some(exe), home))
}

/// Pure resolver, testable in isolation.
pub(crate) fn resolve_skills_dir(
    env_override: Option<String>,
    current_exe: Option<PathBuf>,
    home: PathBuf,
) -> PathBuf {
    if let Some(p) = env_override {
        return PathBuf::from(p);
    }
    if let Some(exe) = current_exe {
        if let Some(parent) = exe.parent() {
            let candidate = parent.join("../.agents/skills");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    home.join(".rootcx").join("skills")
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn none_tokens_are_omitted_from_json() {
        let c = Config { url: "http://x".into(), token: None, refresh_token: None };
        let j = serde_json::to_string(&c).unwrap();
        assert!(!j.contains("token"), "got: {j}");
        assert!(j.contains("\"url\""));
    }

    #[test]
    fn some_tokens_roundtrip() {
        let c = Config {
            url: "http://x".into(),
            token: Some("a".into()),
            refresh_token: Some("r".into()),
        };
        let j = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&j).unwrap();
        assert_eq!(back.token.as_deref(), Some("a"));
        assert_eq!(back.refresh_token.as_deref(), Some("r"));
    }

    #[test]
    fn loads_legacy_config_without_refresh_token() {
        let j = r#"{"url":"http://x","token":"a"}"#;
        let c: Config = serde_json::from_str(j).unwrap();
        assert!(c.refresh_token.is_none());
    }

    // merge_config — tokens must survive a no-op re-connect to the same URL.
    // This is the exact bug that made the user lose their session after /rootcx:connect.

    use super::merge_config;

    fn cfg(url: &str, t: Option<&str>, r: Option<&str>) -> Config {
        Config {
            url: url.into(),
            token: t.map(Into::into),
            refresh_token: r.map(Into::into),
        }
    }

    #[test]
    fn merge_first_connect_has_no_tokens() {
        let c = merge_config(None, "http://x".into(), None);
        assert_eq!(c.url, "http://x");
        assert!(c.token.is_none());
        assert!(c.refresh_token.is_none());
    }

    #[test]
    fn merge_reconnect_same_url_preserves_tokens() {
        let existing = cfg("http://x", Some("a"), Some("r"));
        let c = merge_config(Some(existing), "http://x".into(), None);
        assert_eq!(c.token.as_deref(), Some("a"));
        assert_eq!(c.refresh_token.as_deref(), Some("r"));
    }

    #[test]
    fn merge_normalizes_trailing_slash_and_still_preserves() {
        let existing = cfg("http://x", Some("a"), Some("r"));
        let c = merge_config(Some(existing), "http://x/".into(), None);
        assert_eq!(c.url, "http://x");
        assert_eq!(c.token.as_deref(), Some("a"));
    }

    #[test]
    fn merge_url_change_drops_tokens() {
        let existing = cfg("http://x", Some("a"), Some("r"));
        let c = merge_config(Some(existing), "http://y".into(), None);
        assert!(c.token.is_none());
        assert!(c.refresh_token.is_none());
    }

    #[test]
    fn merge_explicit_token_replaces_and_clears_refresh() {
        let existing = cfg("http://x", Some("old"), Some("oldr"));
        let c = merge_config(Some(existing), "http://x".into(), Some("new".into()));
        assert_eq!(c.token.as_deref(), Some("new"));
        assert!(c.refresh_token.is_none(), "new manual token invalidates refresh");
    }

    // resolve_skills_dir — env > repo-relative > home fallback

    use super::resolve_skills_dir;
    use std::path::PathBuf;

    #[test]
    fn skills_env_override_wins() {
        let p = resolve_skills_dir(
            Some("/tmp/override".into()),
            Some(PathBuf::from("/ignored/bin/rootcx")),
            PathBuf::from("/home/u"),
        );
        assert_eq!(p, PathBuf::from("/tmp/override"));
    }

    #[test]
    fn skills_falls_back_to_home_when_nothing_matches() {
        let p = resolve_skills_dir(
            None,
            Some(PathBuf::from("/nonexistent/bin/rootcx")),
            PathBuf::from("/home/u"),
        );
        assert_eq!(p, PathBuf::from("/home/u/.rootcx/skills"));
    }

    #[test]
    fn skills_picks_repo_relative_when_candidate_exists() {
        // Create a fake layout: <tmp>/bin/rootcx + <tmp>/.agents/skills/
        let base = std::env::temp_dir().join(format!("rootcx-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::create_dir_all(base.join(".agents/skills")).unwrap();
        let exe = base.join("bin/rootcx");
        std::fs::write(&exe, "").unwrap();

        let p = resolve_skills_dir(None, Some(exe), PathBuf::from("/home/u"));
        // candidate = <tmp>/bin/../.agents/skills — verify it resolves to our fake dir
        assert!(p.ends_with(".agents/skills"));
        assert!(p.exists());
    }
}
