pub mod refs;

use std::fmt::Write;

use chromiumoxide::Page;
use serde::{Deserialize, Serialize};

use crate::error::BrowserError;
pub use refs::RefRegistry;

const JS: &str = include_str!("extract.js");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub url: String,
    pub title: String,
    pub text: String,
    pub refs: RefRegistry,
}

#[derive(Deserialize)]
struct El { idx: u32, kind: String, role: String, name: String, sel: String }

pub async fn take(page: &Page) -> Result<Snapshot, BrowserError> {
    let url = page.url().await?.unwrap_or_default();
    let title = page.get_title().await?.unwrap_or_default();

    let raw: String = page.evaluate(JS).await
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
                refs.insert(e.idx, e.sel, e.role, e.name);
            }
            "l" => { let _ = writeln!(text, "-- {}: {} --", e.role, e.name); }
            _ => {}
        }
    }

    Ok(Snapshot { url, title, text, refs })
}
