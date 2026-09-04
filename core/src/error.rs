#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("database connection error: {0}")]
    Database(sqlx::Error),

    #[error("schema migration error: {0}")]
    Schema(sqlx::Error),

    /// A manifest the Core refuses to install. Distinct from `Schema` because the
    /// author has to be told what to fix: wrapped as a `sqlx` error it mapped to
    /// `500 internal error`, which discards the message the validator built.
    #[error("invalid manifest: {0}")]
    Invalid(String),

    #[error("secret vault error: {0}")]
    Secret(String),

    #[error("worker error: {0}")]
    Worker(String),

    /// The pod cannot host another worker process right now. A transient
    /// condition the caller should retry, not a fault: mapping it to 500 would
    /// tell an operator to look for a bug and a client never to try again.
    #[error("{0}")]
    Capacity(String),

    #[error("job engine error: {0}")]
    Job(String),

    #[error("cron error: {0}")]
    Cron(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("MCP error: {0}")]
    Mcp(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("delegation refused: {0}")]
    Delegation(String),
}
