pub mod bin;
pub mod bundle;
pub mod dirs;
pub mod env;
pub mod fs;
pub mod process;
pub mod service;
pub mod shell;

pub const DEFAULT_API_PORT: u16 = 9100;

#[derive(Debug, thiserror::Error)]
#[error("could not determine {0}")]
pub struct PlatformError(pub &'static str);
