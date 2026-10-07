//! Level-cap projection (Phase 8): a character above a realm's level cap is shown to the realm as a **working copy at the cap**, and what that
//! copy cannot hold stays in the canonical character.
//!
//! ```text
//!   C   canonical, never down-levelled (the Owner's truth)
//!   P   = apply(hold, C)      what the realm is given: level = cap, xp 0, minus what the core *holds* at that level
//!   B0, B1   what the realm shows (a projected view)
//!   C'  = merge3(C, B0, B1) with level and xp frozen and the build records merged key by key
//! ```
//!
//! **Who decides.** Only the running core knows the effective, post-scaling item levels and holds the CoA progression tables, so the core
//! decides what is held (`portable project`): items that its own loader would take out of the equipment slots and mail, abilities no
//! source grants at the cap, talent picks above the level or the point budget of the cap, the buttons that show such abilities, and the
//! entries of the stored builds. This module only *applies* that decision; it never reads a level requirement.
//!
//! **Why `merge3` keeps what is held.** A held item, spell or button is absent from `B0` and from `B1`, so for the merge it is "filtered away
//! by the realm": it stays canonical. The one exception is a build record the core rewrites at every save (`core.ascension_slot.*`): its
//! whole value differs between `C` and `B0`, so those records are merged key by key ([`settings`]).

pub mod settings;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::capabilities::Progression;
use super::error::{PortableError, Result};
use super::ids::PortableItemId;
use super::model::PortableCharacter;
use super::snapshot;

/// Version of the projection policy this Manager speaks (the core reports its own; a different one is refused).
pub const POLICY_VERSION: u32 = 1;
/// Version of the projection exchange with the core.
pub const PROTOCOL: u32 = 1;

/// What one build record loses at the cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingHold {
    pub source: String,
    pub kind: settings::SettingKind,
    /// Talent entries held (slot, build).
    #[serde(default)]
    pub entries: Vec<u32>,
    /// Buttons held (slot actions, bar).
    #[serde(default)]
    pub buttons: Vec<u32>,
}

/// The core's decision for one canonical character at one cap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionHold {
    pub protocol: u32,
    pub policy_version: u32,
    pub progression_signature: String,
    pub max_player_level: u32,
    pub canonical_level: u32,
    pub projected_level: u32,
    #[serde(default)]
    pub held_items: Vec<PortableItemId>,
    #[serde(default)]
    pub held_spells: Vec<u32>,
    #[serde(default)]
    pub held_actions: Vec<(u8, u8)>,
    #[serde(default)]
    pub settings: Vec<SettingHold>,
    /// Build records the core could not take apart: they are not given to the realm and never merged back.
    #[serde(default)]
    pub blocked_settings: Vec<String>,
    /// The canonical snapshot (its content hash) this decision was made for; empty when it is only a stored decision being re-applied.
    #[serde(default)]
    pub subject: String,
}

/// What the core answers to `portable project`: whether the character is projected at all, and the decision when it is.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionAnswer {
    pub status: String,
    pub projection: AnswerBlock,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnswerBlock {
    pub active: bool,
    pub protocol: u32,
    pub policy_version: u32,
    pub progression_signature: String,
    pub max_player_level: u32,
    pub canonical_level: u32,
    pub projected_level: u32,
    #[serde(default)]
    pub held_items: Vec<PortableItemId>,
    #[serde(default)]
    pub held_spells: Vec<u32>,
    #[serde(default)]
    pub held_actions: Vec<(u8, u8)>,
    #[serde(default)]
    pub settings: Vec<SettingHold>,
    #[serde(default)]
    pub blocked_settings: Vec<String>,
}

/// What a realm decided about a canonical character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The character's level is within the cap: the realm gets it as it is.
    Native,
    Projected(ProjectionHold),
}

impl ProjectionAnswer {
    /// Parse the single JSON line of the console's answer (or the result file) strictly.
    pub fn parse(text: &str, subject: &str) -> Result<Decision> {
        let line = text.lines().map(str::trim).find(|l| l.starts_with('{')).ok_or_else(|| PortableError::Invalid("the core did not answer a projection query".into()))?;
        if line.len() > 8 * 1024 * 1024 {
            return Err(PortableError::LimitExceeded("the core's projection answer is too large".into()));
        }
        if let Ok(refused) = serde_json::from_str::<serde_json::Value>(line) {
            if refused.get("status").and_then(|s| s.as_str()) == Some("refused") {
                let why = refused["problems"].as_array().map(|p| p.iter().map(|x| format!("{}: {}", x["code"].as_str().unwrap_or("?"), x["detail"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("; ")).unwrap_or_default();
                return Err(PortableError::Invalid(format!("the core refused the projection query ({why})")));
            }
        }
        let answer: ProjectionAnswer = serde_json::from_str(line).map_err(|e| PortableError::Invalid(format!("the core's projection answer is not valid: {e}")))?;
        let b = answer.projection;
        if answer.status != "ok" {
            return Err(PortableError::Invalid(format!("the core's projection answer has status {:?}", answer.status)));
        }
        if b.protocol != PROTOCOL || b.policy_version != POLICY_VERSION {
            return Err(PortableError::Invalid(format!("the core projects with protocol {} / policy {}; this Manager speaks {PROTOCOL} / {POLICY_VERSION}", b.protocol, b.policy_version)));
        }
        if !b.active {
            return Ok(Decision::Native);
        }
        if b.projected_level != b.max_player_level || b.canonical_level <= b.max_player_level {
            return Err(PortableError::Invalid("the core's projection is not from the character's level down to the cap".into()));
        }
        Ok(Decision::Projected(ProjectionHold {
            protocol: b.protocol,
            policy_version: b.policy_version,
            progression_signature: b.progression_signature,
            max_player_level: b.max_player_level,
            canonical_level: b.canonical_level,
            projected_level: b.projected_level,
            held_items: b.held_items,
            held_spells: b.held_spells,
            held_actions: b.held_actions,
            settings: b.settings,
            blocked_settings: b.blocked_settings,
            subject: subject.to_string(),
        }))
    }
}

/// Whether a character is projected on a realm, by level alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// The character's level is within the cap.
    None,
    Active { canonical_level: u32, projected_level: u32 },
}

pub fn activation(canonical_level: u8, progression: &Progression) -> Activation {
    if (canonical_level as u32) > progression.max_player_level {
        Activation::Active { canonical_level: canonical_level as u32, projected_level: progression.max_player_level }
    } else {
        Activation::None
    }
}

/// The identity of a canonical snapshot for which a hold was made.
pub fn subject_of(model: &PortableCharacter) -> Result<String> {
    Ok(hex::encode(snapshot::encode(model)?.content_hash))
}

impl ProjectionHold {
    /// A hold applies to the snapshot it was made for.
    pub fn check_subject(&self, model: &PortableCharacter) -> Result<()> {
        if self.subject.is_empty() || self.subject == subject_of(model)? {
            Ok(())
        } else {
            Err(PortableError::Invalid("the projection was made for another state of the character".into()))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.held_items.is_empty() && self.held_spells.is_empty() && self.held_actions.is_empty() && self.settings.iter().all(|s| s.entries.is_empty() && s.buttons.is_empty()) && self.blocked_settings.is_empty()
    }

    /// The pin of this projection (what a session is bound to).
    pub fn pin(&self, content_profile_hash: &str) -> ProgressionPin {
        ProgressionPin { projected: true, max_player_level: self.max_player_level, policy_version: self.policy_version, progression_signature: self.progression_signature.clone(), content_profile_hash: content_profile_hash.to_string() }
    }
}

/// What a character is given on a realm: the canonical character with what the realm cannot hold taken out, at the cap. A hold computed for
/// an earlier state of the character can be applied to a later one (what it names is removed, nothing else is looked at).
pub fn apply(canonical: &PortableCharacter, hold: &ProjectionHold) -> PortableCharacter {
    let mut p = canonical.clone();
    p.progression.level = hold.projected_level.min(u8::MAX as u32) as u8;
    p.progression.xp = 0;
    let held: BTreeSet<PortableItemId> = hold.held_items.iter().copied().collect();
    p.items.retain(|i| !held.contains(&i.id) && !i.container.is_some_and(|c| held.contains(&c)));
    let spells: BTreeSet<u32> = hold.held_spells.iter().copied().collect();
    p.build.spells.retain(|(s, _)| !spells.contains(s));
    let actions: BTreeSet<(u8, u8)> = hold.held_actions.iter().copied().collect();
    p.actions.retain(|a| !actions.contains(&(a.spec, a.button)));
    for source in &hold.blocked_settings {
        p.settings.remove(source);
    }
    for s in &hold.settings {
        let Some(values) = p.settings.get(&s.source) else { continue };
        match settings::Record::parse(s.kind, values) {
            Some(record) => {
                let projected = record.without(&s.entries, &s.buttons).write();
                p.settings.insert(s.source.clone(), projected);
            }
            None => {
                p.settings.remove(&s.source);
            }
        }
    }
    p.normalized()
}

/// What a session or a working copy is bound to. A message under another pin is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgressionPin {
    /// The character is projected (above the cap) as opposed to native.
    pub projected: bool,
    pub max_player_level: u32,
    pub policy_version: u32,
    pub progression_signature: String,
    pub content_profile_hash: String,
}

impl ProgressionPin {
    /// The words the core stores in `coa.portable.pin`: policy version, level cap, then the signature in eight big-endian words.
    pub fn words(&self) -> Vec<u32> {
        let mut words = vec![self.policy_version, self.max_player_level];
        for chunk in self.progression_signature.as_bytes().chunks(8) {
            words.push(u32::from_str_radix(std::str::from_utf8(chunk).unwrap_or("0"), 16).unwrap_or(0));
        }
        words
    }

    pub fn native(progression: &Progression, content_profile_hash: &str) -> Self {
        Self { projected: false, max_player_level: progression.max_player_level, policy_version: progression.projection_policy_version, progression_signature: progression.progression_signature.clone(), content_profile_hash: content_profile_hash.to_string() }
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |what: &str| PortableError::Invalid(format!("progression pin: {what}"));
        if self.max_player_level == 0 || self.max_player_level > 255 || self.policy_version == 0 {
            return Err(bad("a number is out of range"));
        }
        let hex64 = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex64(&self.progression_signature) || !hex64(&self.content_profile_hash) {
            return Err(bad("a hash is not a SHA-256"));
        }
        Ok(())
    }
}

/// Everything remembered of one projected character on one realm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionContext {
    pub canonical_level: u32,
    pub projected_level: u32,
    /// The canonical revision the working copy was last brought to (import, update, or the last acknowledged checkpoint).
    pub canonical_revision: u64,
    pub content_profile_hash: String,
    pub progression_signature: String,
    pub projection_policy_version: u32,
    pub max_player_level: u32,
    pub hold: ProjectionHold,
}

impl ProjectionContext {
    pub fn new(hold: ProjectionHold, canonical_revision: u64, content_profile_hash: &str) -> Self {
        Self {
            canonical_level: hold.canonical_level,
            projected_level: hold.projected_level,
            canonical_revision,
            content_profile_hash: content_profile_hash.to_string(),
            progression_signature: hold.progression_signature.clone(),
            projection_policy_version: hold.policy_version,
            max_player_level: hold.max_player_level,
            hold,
        }
    }

    pub fn pin(&self) -> ProgressionPin {
        self.hold.pin(&self.content_profile_hash)
    }

    pub fn validate(&self) -> Result<()> {
        if self.projected_level >= self.canonical_level || self.projected_level != self.max_player_level || self.hold.progression_signature != self.progression_signature {
            return Err(PortableError::Invalid("the projection context is not consistent".into()));
        }
        self.pin().validate()
    }
}

/// How the core is asked. The running core is the only source of a decision; tests supply their own.
pub trait ProjectionOracle: Send + Sync {
    /// The decision for exactly this canonical character.
    fn decide(&self, canonical: &PortableCharacter) -> Result<Decision>;
}

/// An oracle in the options of an operation.
#[derive(Clone)]
pub struct Oracle(pub std::sync::Arc<dyn ProjectionOracle>);

impl std::fmt::Debug for Oracle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Oracle")
    }
}

/// A decision made earlier by a running core, supplied for an operation on a realm that is stopped. It answers only for the exact
/// canonical snapshot it was made for.
#[derive(Clone, Debug)]
pub struct SuppliedDecision(pub ProjectionHold);

impl ProjectionOracle for SuppliedDecision {
    fn decide(&self, canonical: &PortableCharacter) -> Result<Decision> {
        self.0.check_subject(canonical)?;
        if self.0.subject.is_empty() {
            return Err(PortableError::Invalid("a supplied projection must name the snapshot it was made for".into()));
        }
        Ok(Decision::Projected(self.0.clone()))
    }
}

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
