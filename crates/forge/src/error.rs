#[derive(Debug, thiserror::Error)]
pub enum ForgeError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("provider: {0}")]
    Provider(String),
    #[error("tool `{0}`: {1}")]
    Tool(String, String),
    #[error("stream: {0}")]
    Stream(String),
    #[error("permission rejected")]
    PermissionRejected,
    #[error("aborted")]
    Aborted,
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for ForgeError {
    fn from(e: reqwest::Error) -> Self {
        Self::Provider(e.to_string())
    }
}
