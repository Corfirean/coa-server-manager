//! Portable characters: the local canonical storage of Phase 1.
//!
//! A portable character is owned by this Manager, not by any realm. Realm databases only ever hold temporary working
//! copies. This module contains only the Manager-side model and store; it does not read or write any realm database
//! (that is Phase 2 onwards) and knows nothing about the network.
//!
//! See `docs/PORTABLE_CHARACTERS_AUDIT.md` for the reasoning and `docs/PORTABLE_FORMAT.md` for the format rules.

pub mod capabilities;
pub mod collection;
pub mod compat;
pub mod error;
pub mod extension;
pub mod identity;
pub mod ids;
pub mod merge;
pub mod model;
pub mod projection;
pub mod realm;
pub mod service;
pub mod session;
pub mod snapshot;
pub mod store;
pub mod versions;

#[cfg(test)]
pub(crate) mod fixtures;

pub use collection::IdSet;
pub use error::{PortableError, Result};
pub use ids::{
    CharacterId, ContentId, ImportId, PortableItemId, PortablePetId, ProfileId, SessionId,
};
pub use model::{PortableCharacter, Ruleset};
pub use store::Store;
