#[derive(Debug, thiserror::Error)]
pub enum BrowserError {
    #[error("launch: {0}")]
    Launch(String),
    #[error("navigation: {0}")]
    Navigation(String),
    #[error("ref e{0} not found")]
    ElementNotFound(u32),
    #[error("action: {0}")]
    Action(String),
    #[error("cdp: {0}")]
    Cdp(String),
}

impl From<chromiumoxide::error::CdpError> for BrowserError {
    fn from(e: chromiumoxide::error::CdpError) -> Self {
        Self::Cdp(e.to_string())
    }
}
