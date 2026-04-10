#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("database connection error: {0}")]
    Database(sqlx::Error),

    #[error("schema migration error: {0}")]
    Schema(sqlx::Error),

    #[error("secret vault error: {0}")]
    Secret(String),

    #[error("worker error: {0}")]
    Worker(String),

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
}
