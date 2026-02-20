use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PgError {
    #[error("initdb failed (data_dir={data_dir}): {source}")]
    InitDb { data_dir: PathBuf, source: std::io::Error },

    #[error("initdb exited with status {status}: {stderr}")]
    InitDbFailed { status: i32, stderr: String },

    #[error("pg_ctl start failed: {source}")]
    Start { source: std::io::Error },

    #[error("pg_ctl start exited with status {status}: {stderr}")]
    StartFailed { status: i32, stderr: String },

    #[error("pg_ctl stop failed: {source}")]
    Stop { source: std::io::Error },

    #[error("pg_ctl stop exited with status {status}: {stderr}")]
    StopFailed { status: i32, stderr: String },

    #[error("could not determine user data directory")]
    NoDataDir,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
