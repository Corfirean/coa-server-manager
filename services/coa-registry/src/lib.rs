//! The CoA realm Registry (Phase 10): who publishes a realm and whether it is alive. Nothing else.
//!
//! The Registry never receives or stores database or RA credentials, game account passwords, character snapshots, canonical revisions,
//! inventories, session payloads or player character ids. Every mutating request is signed by the realm's own Ed25519 key.

pub mod api;
pub mod config;
pub mod limits;
pub mod listener;
pub mod pg;
pub mod store;
