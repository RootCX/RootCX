mod error;
mod manager;

pub use error::PgError;
pub use manager::{PostgresManager, data_base_dir};
