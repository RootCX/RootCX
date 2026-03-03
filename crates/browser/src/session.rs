use crate::{action, chrome, error::BrowserError, snapshot, wait};
use crate::snapshot::{Snapshot, SnapshotMode};

pub struct BrowserSession {
    handle: chrome::ChromeHandle,
    last_snapshot: Option<Snapshot>,
}

impl BrowserSession {
    pub async fn launch() -> Result<Self, BrowserError> {
        Ok(Self { handle: chrome::launch().await?, last_snapshot: None })
    }

    pub fn page_url(&self) -> &str {
        self.last_snapshot.as_ref().map_or("", |s| &s.url)
    }

    pub async fn navigate(&mut self, url: &str) -> Result<Snapshot, BrowserError> {
        if let Ok(parsed) = url::Url::parse(url) {
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(BrowserError::Navigation(format!("scheme '{}' not allowed; use http or https", parsed.scheme())));
            }
        }
        wait::navigate(&self.handle.page, url).await
            .map_err(BrowserError::Navigation)?;
        self.snapshot(SnapshotMode::Full).await
    }

    pub async fn snapshot(&mut self, mode: SnapshotMode) -> Result<Snapshot, BrowserError> {
        let snap = snapshot::take(&self.handle.page, mode).await?;
        self.last_snapshot = Some(snap.clone());
        Ok(snap)
    }

    pub async fn click(&mut self, ref_id: u32) -> Result<(), BrowserError> {
        action::click(&self.handle.page, self.refs()?, ref_id).await
    }

    pub async fn type_keys(&mut self, ref_id: u32, text: &str) -> Result<(), BrowserError> {
        action::type_keys(&self.handle.page, self.refs()?, ref_id, text).await
    }

    pub async fn scroll(&mut self, direction: &str, amount: u32) -> Result<(), BrowserError> {
        action::scroll(&self.handle.page, direction, amount).await
    }

    pub async fn press_key(&mut self, key: &str) -> Result<(), BrowserError> {
        action::press_key(&self.handle.page, key).await
    }

    pub async fn select_option(&mut self, ref_id: u32, value: &str) -> Result<(), BrowserError> {
        action::select_option(&self.handle.page, self.refs()?, ref_id, value).await
    }

    pub async fn hover(&mut self, ref_id: u32) -> Result<(), BrowserError> {
        action::hover(&self.handle.page, self.refs()?, ref_id).await
    }

    fn refs(&self) -> Result<&snapshot::refs::RefRegistry, BrowserError> {
        self.last_snapshot.as_ref().map(|s| &s.refs)
            .ok_or_else(|| BrowserError::Action("snapshot required".into()))
    }
}
