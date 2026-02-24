use std::path::PathBuf;

use chromiumoxide::browser::BrowserConfig;
use chromiumoxide::cdp::browser_protocol::page::EnableParams;
use chromiumoxide::{Browser, Page};
use futures::StreamExt;
use tokio::task::JoinHandle;

use crate::error::BrowserError;

pub struct ChromeHandle {
    pub page: Page,
    _browser: Browser,
    _handler: JoinHandle<()>,
}

pub async fn launch() -> Result<ChromeHandle, BrowserError> {
    let mut builder = BrowserConfig::builder()
        .with_head()
        .window_size(1280, 900)
        .arg("--disable-blink-features=AutomationControlled")
        .arg("--no-first-run")
        .arg("--disable-dev-shm-usage");

    if let Some(path) = find_binary() {
        builder = builder.chrome_executable(path);
    }

    let config = builder.build().map_err(BrowserError::Launch)?;
    let (browser, mut handler) =
        Browser::launch(config).await.map_err(|e| BrowserError::Launch(e.to_string()))?;

    let h = tokio::spawn(async move { while handler.next().await.is_some() {} });

    let page = browser
        .new_page("about:blank")
        .await
        .map_err(|e| BrowserError::Launch(e.to_string()))?;

    page.enable_stealth_mode().await.map_err(|e| BrowserError::Launch(e.to_string()))?;
    // Lifecycle events for wait::network_idle
    page.execute(EnableParams::default()).await.map_err(|e| BrowserError::Launch(e.to_string()))?;

    Ok(ChromeHandle { page, _browser: browser, _handler: h })
}

fn find_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_PATH") {
        let path = PathBuf::from(p);
        if path.exists() { return Some(path); }
    }
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    } else if cfg!(target_os = "linux") {
        &["/usr/bin/google-chrome", "/usr/bin/chromium-browser"]
    } else {
        &[r"C:\Program Files\Google\Chrome\Application\chrome.exe"]
    };
    candidates.iter().map(PathBuf::from).find(|p| p.exists())
}
