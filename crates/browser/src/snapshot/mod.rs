pub mod a11y;
pub mod refs;

use std::fmt::Write;

use chromiumoxide::Page;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::error::BrowserError;
pub use refs::RefRegistry;

const JS_FALLBACK: &str = include_str!("extract.js");

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub enum SnapshotMode { #[default] Full, Efficient }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub url: String,
    pub title: String,
    pub text: String,
    pub refs: RefRegistry,
}

pub async fn take(page: &Page, mode: SnapshotMode) -> Result<Snapshot, BrowserError> {
    let url = page.url().await?.unwrap_or_default();
    let title = page.get_title().await?.unwrap_or_default();

    let cfg = match mode {
        SnapshotMode::Full => a11y::ExtractConfig::full(),
        SnapshotMode::Efficient => a11y::ExtractConfig::efficient(),
    };

    match a11y::extract(page, &cfg).await {
        Ok((text, refs)) if !refs.is_empty() => {
            debug!("a11y snapshot: {} refs, {} chars", refs.len(), text.len());
            return Ok(Snapshot { url, title, text, refs });
        }
        Ok(_) => debug!("a11y tree empty, falling back to JS"),
        Err(e) => debug!("a11y failed ({e}), falling back to JS"),
    }

    take_js(page, url, title).await
}

#[derive(Deserialize)]
struct El { idx: u32, kind: String, role: String, name: String, sel: String }

async fn take_js(page: &Page, url: String, title: String) -> Result<Snapshot, BrowserError> {
    let raw: String = page.evaluate(JS_FALLBACK).await
        .map_err(|e| BrowserError::Cdp(e.to_string()))?
        .into_value()
        .map_err(|e| BrowserError::Cdp(format!("{e:?}")))?;

    let els: Vec<El> = serde_json::from_str(&raw).unwrap_or_default();
    let mut text = String::with_capacity(8192);
    let mut refs = RefRegistry::default();

    for e in els {
        match e.kind.as_str() {
            "i" => {
                let _ = writeln!(text, "[e{}] {} \"{}\"", e.idx, e.role, e.name);
                refs.insert(e.idx, e.role, e.name, Some(e.sel), None);
            }
            "l" => { let _ = writeln!(text, "-- {}: {} --", e.role, e.name); }
            _ => {}
        }
    }

    Ok(Snapshot { url, title, text, refs })
}
