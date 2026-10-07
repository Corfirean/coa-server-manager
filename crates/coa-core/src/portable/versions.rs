//! Format and protocol versions. They exist from the first commit so that nothing has to be guessed later.
//!
//! A reader refuses data whose version is **newer** than it knows; older versions are read through a migration.
//! The registry and relay versions are not used by anything yet (Phases 10 and 13); they live here so there is one
//! place that lists every version the portable system speaks.

/// Version of the serialised [`PortableCharacter`](super::model::PortableCharacter) this build **writes**.
///
/// * 1: Phases 1-5.
/// * 2: Phase 6 added the `wardrobe` section. A version 1 payload is read through a strict migration (see `snapshot.rs`): it must
///   not contain a `wardrobe`, its original hash is verified before it is migrated, and it becomes a character with an empty one.
pub const PORTABLE_CHARACTER_FORMAT_VERSION: u32 = 2;
/// The oldest character format this build can still **read**.
pub const PORTABLE_CHARACTER_MIN_READ_VERSION: u32 = 1;
/// Version of the header + body of an online import job (`<job_id>.job`), checked by the core before it parses the body.
/// * 1: Phase 5 (no version field; never accepted by a core that knows versions).
/// * 2: the explicit `job_format` field, body = character format 2 (optional `wardrobe`).
pub const ONLINE_IMPORT_JOB_FORMAT_VERSION: u32 = 2;
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
