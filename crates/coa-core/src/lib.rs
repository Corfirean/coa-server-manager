pub mod backup;
pub mod config;
pub mod db;
pub mod driver;
pub mod error;
pub mod fsx;
pub mod health;
pub mod layout;
pub mod logging;
pub mod process;
pub mod manifest;
pub mod registry;
pub mod signing;

pub use error::{Error, ErrorCode, Result};

pub const MANAGER_VERSION: &str = env!("CARGO_PKG_VERSION");
