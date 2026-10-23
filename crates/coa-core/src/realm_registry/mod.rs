//! Publishing a realm to the central Registry (Phase 10): the Host side of registration and presence.
//!
//! The Registry is discovery infrastructure only. What this module sends is the realm's public identity (a UUIDv7 and an Ed25519 public key), a
//! display name, a description, a language, the Manager version, the realm's advertised capabilities and optionally its player count. It never
//! sends or reads a database or console credential, a game account, a character, a canonical revision or a session payload, and it never changes
//! portable state. See `docs/REGISTRY_PROTOCOL.md`.

pub mod advert;
pub mod browse;
pub mod client;
pub mod host;
pub mod keys;
pub mod runtime;
pub mod settings;

pub use advert::{advertise, AdvertSource, LocalAdvert, LocalRealmsSource};
pub use browse::{BrowseClient, BrowseParams};
pub use client::{ClientError, RegistryClient};
pub use host::{PublishState, RealmPublishStatus, RegistryHost, RegistryStatus, Timing};
pub use keys::{FileKeyStore, KeyStore};
pub use runtime::RegistryRuntime;
