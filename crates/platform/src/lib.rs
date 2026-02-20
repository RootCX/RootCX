pub mod bin;
pub mod dirs;
pub mod env;
pub mod fs;
pub mod process;
pub mod shell;

#[derive(Debug, thiserror::Error)]
#[error("could not determine {0}")]
pub struct PlatformError(pub &'static str);
