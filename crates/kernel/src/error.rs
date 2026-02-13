use rootcx_postgres_mgmt::PgError;

#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("postgres management error: {0}")]
    Postgres(#[from] PgError),

    #[error("database connection error: {0}")]
    Database(sqlx::Error),

    #[error("schema migration error: {0}")]
    Schema(sqlx::Error),
}
