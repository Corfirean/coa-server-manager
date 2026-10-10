#![allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::doc_overindented_list_items,
    clippy::unnecessary_sort_by,
    clippy::explicit_counter_loop,
    clippy::large_enum_variant,
)]

pub mod accounts;
pub mod allsettings;
pub mod backup;
pub mod cleanbase;
pub mod client;
pub mod clientdl;
pub mod companions;
pub mod config;
pub mod console;
pub mod control;
pub mod crashes;
pub mod custom_races;
pub mod dashboard;
pub mod db;
pub mod diag;
pub mod docker;
pub mod download;
pub mod driver;
pub mod error;
pub mod firewall;
pub mod friends;
pub mod fsx;
pub mod health;
pub mod install;
pub mod layout;
pub mod logging;
pub mod manifest;
pub mod migrations;
pub mod modules;
pub mod multiworld;
pub mod net;
pub mod package;
pub mod pkgsource;
pub mod platform;
pub mod population;
pub mod portable;
pub mod process;
pub mod ra;
pub mod realm_registry;
pub mod realmlist;
pub mod realms;
pub mod registry;
pub mod release;
pub mod release_schema;
pub mod remote_client;
pub mod repair;
pub mod report;
pub mod schema_check;
pub mod signing;
pub mod squid;
pub mod srp6;
pub mod update;
pub mod upnp;
pub mod wine;

pub use error::{Error, ErrorCode, Result};

pub const MANAGER_VERSION: &str = env!("CARGO_PKG_VERSION");
