pub mod backup;
pub mod config;
pub mod db;
pub mod download;
pub mod driver;
pub mod error;
pub mod fsx;
pub mod health;
pub mod install;
pub mod layout;
pub mod logging;
pub mod pkgsource;
pub mod process;
pub mod manifest;
pub mod migrations;
pub mod package;
pub mod ra;
pub mod registry;
pub mod update;
pub mod signing;

pub use error::{Error, ErrorCode, Result};

pub const MANAGER_VERSION: &str = env!("CARGO_PKG_VERSION");
