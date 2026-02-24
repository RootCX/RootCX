use crate::{action, chrome, error::BrowserError, snapshot, wait};
use crate::snapshot::Snapshot;

pub struct BrowserSession {
    handle: chrome::ChromeHandle,
    last_snapshot: Option<Snapshot>,
}

impl BrowserSession {
    pub async fn launch() -> Result<Self, BrowserError> {
        Ok(Self { handle: chrome::launch().await?, last_snapshot: None })
    }

    pub async fn navigate(&mut self, url: &str) -> Result<Snapshot, BrowserError> {
        self.handle.page.goto(url).await.map_err(|e| BrowserError::Navigation(e.to_string()))?;
        wait::until_ready(&self.handle.page).await;
        self.snapshot().await
    }

    pub async fn snapshot(&mut self) -> Result<Snapshot, BrowserError> {
        let snap = snapshot::take(&self.handle.page).await?;
        self.last_snapshot = Some(snap.clone());
        Ok(snap)
    }

    pub async fn click(&mut self, ref_id: u32) -> Result<Snapshot, BrowserError> {
        action::click(&self.handle.page, self.refs()?, ref_id).await?;
        wait::until_ready(&self.handle.page).await;
        self.snapshot().await
    }

    pub async fn type_keys(&mut self, ref_id: u32, text: &str) -> Result<Snapshot, BrowserError> {
        action::type_keys(&self.handle.page, self.refs()?, ref_id, text).await?;
        wait::until_ready(&self.handle.page).await;
        self.snapshot().await
    }

    pub async fn scroll(&mut self, direction: &str, amount: u32) -> Result<Snapshot, BrowserError> {
        action::scroll(&self.handle.page, direction, amount).await?;
        wait::until_ready(&self.handle.page).await;
        self.snapshot().await
    }

    fn refs(&self) -> Result<&snapshot::refs::RefRegistry, BrowserError> {
        self.last_snapshot
            .as_ref()
            .map(|s| &s.refs)
            .ok_or_else(|| BrowserError::Action("snapshot required".into()))
    }
}
