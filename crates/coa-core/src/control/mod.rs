//! The control plane between a Player Manager and a Host Manager (Phase 12). Account provisioning, character claiming and the route are decided here; nothing
//! here carries game traffic. See `docs/CONTROL_PROTOCOL.md`.

pub mod backend;
pub mod host_link;
pub mod host_manager;
pub mod identity;
pub mod join;
pub mod player;
pub mod secrets;
pub mod service;
pub mod store;
pub mod transport;
