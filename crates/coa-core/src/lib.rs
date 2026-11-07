#![allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::doc_overindented_list_items,
    clippy::unnecessary_sort_by,
    clippy::explicit_counter_loop,
    clippy::large_enum_variant
)]

pub mod accounts;
pub mod allsettings;
pub mod analyzer;
pub mod backup;
pub mod build_cache;
pub mod build_engine;
pub mod catalogue;
pub mod channels;
pub mod character_start_check;
pub mod cleanbase;
pub mod client;
pub mod client_graphics;
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
pub mod glue_config;
pub mod health;
pub mod install;
pub mod layout;
pub mod logging;
pub mod server_logging;
pub mod manifest;
pub mod migrations;
pub mod modular_copy;
pub mod modules;
pub mod module_sources;
pub mod msvc_setup;
pub mod multiworld;
mod mysql_snapshot;
pub mod net;
pub mod package;
pub mod pkgsource;
pub mod platform;
pub mod plugins;
pub mod population;
pub mod portable;
pub mod preview;
pub mod process;
pub mod provenance;
pub mod ra;
pub mod race_availability;
pub mod realm_registry;
pub mod realmlist;
pub mod realms;
pub mod rebuild;
pub mod registry;
pub mod release;
pub mod release_history;
pub mod release_schema;
pub mod remote_client;
pub mod repair;
pub mod report;
pub mod schema_check;
pub mod server_migration;
pub mod signing;
pub mod solver;
pub mod squid;
pub mod srp6;
pub mod update;
mod update_isolation;
pub mod update_route;
pub mod upnp;
pub mod wine;

pub use error::{Error, ErrorCode, Result};

pub const MANAGER_VERSION: &str = env!("CARGO_PKG_VERSION");
