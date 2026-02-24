mod action;
mod chrome;
pub mod error;
pub mod session;
pub mod snapshot;
mod wait;

pub use error::BrowserError;
pub use session::BrowserSession;
pub use snapshot::{Snapshot, SnapshotMode, refs::RefRegistry};
