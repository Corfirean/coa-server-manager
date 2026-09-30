pub mod error;
pub mod fsx;
pub mod logging;
pub mod manifest;
pub mod registry;

pub use error::{Error, ErrorCode, Result};

pub const MANAGER_VERSION: &str = env!("CARGO_PKG_VERSION");
