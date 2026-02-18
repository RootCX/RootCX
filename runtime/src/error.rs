use rootcx_postgres_mgmt::PgError;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("postgres management error: {0}")]
    Postgres(#[from] PgError),

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

    #[error("IPC error: {0}")]
    Ipc(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
