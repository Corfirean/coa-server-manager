//! The wire protocol of the CoA realm Registry, version 2 (Phase 10.1; version 1 was the unstructured Phase-10 form and is refused, see
//! `docs/REGISTRY_PROTOCOL.md`).
//!
//! The Registry is discovery and presence infrastructure only. Everything in this crate is public, host-advertised metadata:
//! it never carries a credential, a character, a snapshot or a session payload. The protocol version is the Registry's own and
//! is unrelated to the portable character, session or capability-profile versions.
//!
//! * [`sign`]: the canonical signed-request input and Ed25519 helpers (see `docs/REGISTRY_PROTOCOL.md`).
//! * [`wire`]: request and response bodies with their limits and validation.
//! * [`caps`]: the strict, bounded form of `RealmCapabilities` a realm advertises.

pub mod browse;
pub mod caps;
pub mod sign;
pub mod wire;

pub use browse::*;
pub use caps::AdvertisedCapabilities;
pub use sign::{RealmId, SignedHeaders};
pub use wire::*;

/// The only protocol version this crate speaks.
pub const REGISTRY_PROTOCOL_VERSION: u32 = 2;

/// The largest request body any endpoint accepts.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// A realm's display name, in characters.
pub const MAX_DISPLAY_NAME_CHARS: usize = 80;
/// A realm's description, in UTF-8 bytes.
pub const MAX_DESCRIPTION_BYTES: usize = 1024;
/// The longest language tag.
pub const MAX_LANGUAGE_BYTES: usize = 16;
/// The longest region tag.
pub const MAX_REGION_BYTES: usize = 16;
/// The longest Manager version string.
pub const MAX_VERSION_BYTES: usize = 32;
/// Players, bots or capacity above this are refused as nonsense.
pub const MAX_PLAYER_NUMBER: u32 = 100_000;
/// The most modules one realm lists.
pub const MAX_MODULES: usize = 64;
/// A rate above this is refused.
pub const MAX_RATE: f64 = 1000.0;

/// How often a Host sends a heartbeat.
pub const HEARTBEAT_INTERVAL_SECS: u64 = 30;
/// A realm that has not been heard of for this long is offline (it is never deleted for it).
pub const ONLINE_TTL_SECS: u64 = 120;
/// How far a request's timestamp may be from the Registry's clock.
pub const MAX_CLOCK_SKEW_SECS: i64 = 300;

pub const PATH_PREFIX: &str = "/registry/v2";
/// Version 1 is refused with an explicit error; this prefix is only used to recognise it.
pub const PATH_PREFIX_V1: &str = "/registry/v1";
pub const PATH_REGISTER: &str = "/registry/v2/realms/register";
pub const PATH_HEALTHZ: &str = "/registry/v2/healthz";
pub const PATH_LIST: &str = "/registry/v2/realms";

pub fn path_heartbeat(realm: &RealmId) -> String {
    format!("{PATH_PREFIX}/realms/{realm}/heartbeat")
}
pub fn path_unpublish(realm: &RealmId) -> String {
    format!("{PATH_PREFIX}/realms/{realm}/unpublish")
}
/// The authenticated (signed) read of a realm's own record, whether or not it is published.
pub fn path_self(realm: &RealmId) -> String {
    format!("{PATH_PREFIX}/realms/{realm}/self")
}
/// The public record of a published realm.
pub fn path_detail(realm: &RealmId) -> String {
    format!("{PATH_PREFIX}/realms/{realm}")
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtoError {
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ProtoError>;

pub(crate) fn invalid<T>(what: impl Into<String>) -> Result<T> {
    Err(ProtoError::Invalid(what.into()))
}
