//! Format and protocol versions. They exist from the first commit so that nothing has to be guessed later.
//!
//! A reader refuses data whose version is **newer** than it knows; older versions are read through a migration.
//! The registry and relay versions are not used by anything yet (Phases 10 and 13); they live here so there is one
//! place that lists every version the portable system speaks.

/// Version of the serialised [`PortableCharacter`](super::model::PortableCharacter).
pub const PORTABLE_CHARACTER_FORMAT_VERSION: u32 = 1;
/// Version of the serialised collection set ([`IdSet`](super::collection::IdSet)).
pub const PORTABLE_COLLECTION_FORMAT_VERSION: u32 = 1;
/// Version of the extension container ([`Extension`](super::model::Extension)).
pub const EXTENSION_FORMAT_VERSION: u32 = 1;
/// Version of the snapshot envelope (header + compressed payload) that is stored and, later, transferred.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;
/// Registry API protocol version (Phase 10).
pub const REGISTRY_PROTOCOL_VERSION: u32 = 1;
/// Relay protocol version (Phase 13).
pub const RELAY_PROTOCOL_VERSION: u32 = 1;
