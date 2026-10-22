//! The portable character: a *semantic* description of what moves between realms (see
//! `docs/PORTABLE_CHARACTERS_AUDIT.md`, sections 5-7). No database row, guid or position of any realm appears here:
//! local guids live in the Manager's mapping tables, and every item/pet carries a global UUIDv7 instead.
//!
//! Rules:
//! * Unknown fields are refused (`deny_unknown_fields`); a change of shape bumps `format_version`.
//! * Numeric ids of game content other than race/class/item/pet entries (spells, quests, factions, skills, ...) are
//!   interpreted in `content_namespace`; race, class and the entries of items/pets carry their own namespace.
//! * Data of modules this Manager does not understand is kept as opaque [`Extension`]s and is never dropped.
//! * Everything is bounded (`limits`), because a snapshot may come from another person's Manager.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::{PortableError, Result};
use super::ids::{valid_namespace, CharacterId, ContentId, PortableItemId, PortablePetId};
use super::versions::PORTABLE_CHARACTER_FORMAT_VERSION;

pub mod limits {
    pub const MAX_NAME_CHARS: usize = 25;
    pub const MAX_PET_NAME_CHARS: usize = 21;
    pub const MAX_ITEMS: usize = 1_500;
    pub const MAX_SLOT: u8 = 149;
    pub const MAX_ENCHANT_SLOTS: u8 = 12;
    pub const MAX_CHARGES: usize = 5;
    pub const MAX_ITEM_TEXT_BYTES: usize = 16 * 1024;
    pub const MAX_SPELLS: usize = 20_000;
    pub const MAX_TALENTS: usize = 4_096;
    pub const MAX_SKILLS: usize = 512;
    pub const MAX_GLYPH_SETS: usize = 8;
    pub const MAX_REPUTATIONS: usize = 2_048;
    pub const MAX_ACTIVE_QUESTS: usize = 512;
    pub const MAX_REWARDED_QUESTS: usize = 50_000;
    pub const MAX_ACTIONS: usize = 1_024;
    pub const MAX_PETS: usize = 64;
    pub const MAX_PET_SPELLS: usize = 512;
    pub const MAX_SETTINGS: usize = 4_096;
    pub const MAX_SETTING_VALUES: usize = 4_096;
    pub const MAX_SETTING_SOURCE_BYTES: usize = 128;
    /// Appearance categories are `1..68` in the realm (`APPEARANCE_CATEGORY_COUNT = 69`).
    pub const MAX_APPEARANCE_CATEGORY: u8 = 68;
    pub const MAX_APPEARANCE_OUTFITS: usize = 100;
    pub const MAX_APPEARANCE_OUTFIT_NAME_BYTES: usize = 64;
    pub const MAX_CLIENT_BLOBS: usize = 8;
    pub const MAX_CLIENT_BLOB_BYTES: usize = 64 * 1024;
    pub const MAX_RAW_TEXT_BYTES: usize = 8 * 1024;
    pub const MAX_EXTENSIONS: usize = 64;
    pub const MAX_EXTENSION_BYTES: usize = 1024 * 1024;
    pub const MAX_EXTENSIONS_TOTAL_BYTES: usize = 4 * 1024 * 1024;
    /// What the realm itself allows (`MAX_MONEY_AMOUNT`, `Player.h`).
    pub const MAX_MONEY: u32 = 0x7FFF_FFFF - 1;
    /// Largest decompressed snapshot accepted (decompression-bomb guard).
    pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
    /// Largest compressed snapshot accepted.
    pub const MAX_COMPRESSED_BYTES: usize = 8 * 1024 * 1024;
}

use limits::*;

/// A character belongs to exactly one ruleset for its whole life; CoA and Wildcard never mix.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ruleset {
    Coa,
    Wildcard,
}

impl Ruleset {
    pub fn as_str(self) -> &'static str {
        match self {
            Ruleset::Coa => "coa",
            Ruleset::Wildcard => "wildcard",
        }
    }
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "coa" => Ok(Ruleset::Coa),
            "wildcard" => Ok(Ruleset::Wildcard),
            other => Err(PortableError::Invalid(format!("unknown ruleset {other:?}"))),
        }
    }
}

impl std::fmt::Display for Ruleset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Binary data as base64 text in the serialised form.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Bytes(pub Vec<u8>);

impl std::fmt::Debug for Bytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bytes({} bytes)", self.0.len())
    }
}

impl Serialize for Bytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use base64::Engine;
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use base64::Engine;
        let text = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .map(Bytes)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub skin: u8,
    pub face: u8,
    pub hair_style: u8,
    pub hair_color: u8,
    pub facial_style: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub name: String,
    pub race: ContentId,
    pub class: ContentId,
    pub gender: u8,
    pub appearance: Appearance,
    /// Only the cosmetic bits of `playerFlags` (helm/cloak visibility); never GM/ghost/AFK bits.
    pub cosmetic_flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Honor {
    pub arena_points: u32,
    pub total_honor: u32,
    pub today_honor: u32,
    pub yesterday_honor: u32,
    pub total_kills: u32,
    pub today_kills: u16,
    pub yesterday_kills: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progression {
    pub level: u8,
    pub xp: u32,
    /// Copper.
    pub money: u32,
    pub honor: Honor,
    pub known_currencies: u64,
    pub chosen_title: u32,
    /// Raw `knownTitles` text of the realm (space separated integers).
    pub known_titles: String,
    /// Raw `exploredZones` text.
    pub explored_zones: String,
    /// Raw `taximask` text.
    pub taxi_mask: String,
    pub bank_slots: u8,
    pub stable_slots: u8,
    pub watched_faction: u32,
    /// Which action bars are shown (bitmask); not the bars themselves.
    pub action_bars_mask: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skill {
    pub skill: u32,
    pub value: u16,
    pub max: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlyphSet {
    pub talent_group: u8,
    pub glyphs: [u16; 6],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    /// `(spell id, spec mask)`, ascending by spell id. CoA talents are spells, so this is also the talent state.
    pub spells: Vec<(u32, u8)>,
    /// Stock talents `(spell id, spec mask)`; empty on CoA, kept for stock-shaped rulesets.
    pub talents: Vec<(u32, u8)>,
    pub skills: Vec<Skill>,
    pub glyphs: Vec<GlyphSet>,
    pub talent_groups_count: u8,
    pub active_talent_group: u8,
    pub extra_bonus_talent_count: i32,
    pub reset_talents_cost: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enchantment {
    /// 0..12, the realm's enchantment slot.
    pub slot: u8,
    pub id: u32,
    pub duration: u32,
    pub charges: u32,
}

/// A wrapped gift: what is inside the wrapper.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gift {
    pub entry: ContentId,
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableItem {
    pub id: PortableItemId,
    /// The bag this item is in; `None` = directly on the character (equipment, bag slots, backpack, bank, keyring,
    /// currency tokens). A container must itself be directly on the character.
    pub container: Option<PortableItemId>,
    pub slot: u8,
    pub entry: ContentId,
    pub count: u32,
    pub duration: i32,
    /// Spell charges, at most five.
    pub charges: Vec<i32>,
    pub flags: u32,
    /// Only the non-empty slots, ascending by slot.
    pub enchantments: Vec<Enchantment>,
    /// Negative = suffix.
    pub random_property_id: i32,
    pub durability: u32,
    pub played_time: u32,
    pub text: Option<String>,
    /// Display name of the crafter; the crafter's guid is never carried (it names a character of another realm).
    pub creator_name: Option<String>,
    pub gift: Option<Gift>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveQuest {
    pub quest: u32,
    pub status: u8,
    pub explored: bool,
    pub timer: u32,
    pub mob_counts: [u16; 4],
    pub item_counts: [u16; 6],
    pub player_count: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quests {
    pub active: Vec<ActiveQuest>,
    /// Completed quest ids, ascending. Daily/weekly/monthly/seasonal state is realm-clock state and is not carried.
    pub rewarded: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReputationEntry {
    pub faction: u32,
    pub standing: i32,
    pub flags: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionButton {
    pub spec: u8,
    pub button: u8,
    pub action: u32,
    pub kind: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PetSpell {
    pub spell: u32,
    pub active: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortablePet {
    pub id: PortablePetId,
    pub entry: ContentId,
    pub model_id: u32,
    pub created_by_spell: u32,
    /// 0 = summon pet, 1 = hunter pet.
    pub pet_type: u8,
    pub level: u16,
    pub exp: u32,
    pub react_state: u8,
    pub name: String,
    pub renamed: bool,
    /// The realm's slot: current / stable 1-4 / not in slot.
    pub slot: u8,
    pub health: u32,
    pub mana: u32,
    pub happiness: u32,
    pub action_bar: String,
    /// Ascending by spell id.
    pub spells: Vec<PetSpell>,
    /// The five declined-name forms (ruRU) when present.
    pub declined_names: Option<[String; 5]>,
}

/// What a character looks like by choice (Phase 6): the **selected** appearance per category, the two visibility switches
/// and the saved outfits. All ids are appearance ids of the character's `content_namespace` (for CoA: `Appearances.dbc`).
///
/// This is selection state only. What the account has *unlocked* is a profile collection (`coa:appearance`, `coa:vanity`),
/// and no gameplay item is referenced from here, so reconciling it can never create or delete an item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableAppearance {
    /// Category (`character_appearance.category_id`, 1..=68) -> appearance id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub active: BTreeMap<u8, u32>,
    /// `character_appearance_settings.can_see_item`; a missing row means `true`.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub can_see_item: bool,
    /// `character_appearance_settings.can_see_spell`; a missing row means `true`.
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub can_see_spell: bool,
    /// Saved outfits by name: the appearance ids in the order the realm stores them (0 = nothing in that category).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outfits: BTreeMap<String, Vec<u32>>,
}

fn yes() -> bool {
    true
}

fn is_yes(v: &bool) -> bool {
    *v
}

impl Default for PortableAppearance {
    fn default() -> Self {
        Self {
            active: BTreeMap::new(),
            can_see_item: true,
            can_see_spell: true,
            outfits: BTreeMap::new(),
        }
    }
}

impl PortableAppearance {
    /// Nothing selected, nothing saved, both switches at the realm default: serialised as absent, so a character without an
    /// appearance has exactly the bytes (and hash) it had before this section existed.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty() && self.outfits.is_empty() && self.can_see_item && self.can_see_spell
    }

    /// Every appearance id mentioned anywhere (selection and outfits), without 0.
    pub fn ids(&self) -> BTreeSet<u32> {
        self.active
            .values()
            .chain(self.outfits.values().flatten())
            .copied()
            .filter(|id| *id != 0)
            .collect()
    }

    /// The part of this appearance a destination that knows `known` can hold: a selection of an unknown id and an outfit
    /// that mentions one are left out (they stay in the canonical character, see `PORTABLE_APPEARANCE.md`).
    pub fn restricted_to(&self, known: impl Fn(u32) -> bool) -> PortableAppearance {
        PortableAppearance {
            active: self
                .active
                .iter()
                .filter(|(_, id)| known(**id))
                .map(|(c, id)| (*c, *id))
                .collect(),
            can_see_item: self.can_see_item,
            can_see_spell: self.can_see_spell,
            outfits: self
                .outfits
                .iter()
                .filter(|(_, ids)| ids.iter().all(|id| *id == 0 || known(*id)))
                .map(|(n, ids)| (n.clone(), ids.clone()))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientBlob {
    pub time: u32,
    pub data: Bytes,
}

/// Data of a module this Manager may not understand. It is stored and returned byte for byte.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    pub module_version: String,
    pub format_version: u32,
    /// Lower-case hex SHA-256 of `payload`; a mismatch means the blob was corrupted.
    pub content_hash: String,
    pub payload: Bytes,
}

impl Extension {
    pub fn new(module_version: &str, format_version: u32, payload: Vec<u8>) -> Self {
        Self {
            module_version: module_version.to_string(),
            format_version,
            content_hash: hex::encode(Sha256::digest(&payload)),
            payload: Bytes(payload),
        }
    }

    pub fn is_intact(&self) -> bool {
        self.content_hash == hex::encode(Sha256::digest(&self.payload.0))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableCharacter {
    pub format_version: u32,
    pub character_id: CharacterId,
    pub ruleset: Ruleset,
    /// Namespace of the plain numeric ids below (spells, quests, factions, skills, glyphs).
    pub content_namespace: String,
    pub identity: Identity,
    pub progression: Progression,
    pub build: Build,
    pub items: Vec<PortableItem>,
    pub quests: Quests,
    pub reputation: Vec<ReputationEntry>,
    pub actions: Vec<ActionButton>,
    pub pets: Vec<PortablePet>,
    /// CoA gameplay state (`character_settings`): source -> integer vector. Only allow-listed sources are carried
    /// (policy lives with the exporter).
    pub settings: BTreeMap<String, Vec<u32>>,
    /// Selected appearances, visibility switches and saved outfits (absent in snapshots made before Phase 6).
    #[serde(default, skip_serializing_if = "PortableAppearance::is_empty")]
    pub wardrobe: PortableAppearance,
    /// Per-character client blobs by account-data type (macros = 5).
    pub client_data: BTreeMap<u8, ClientBlob>,
    /// Module data by extension namespace (`mod:<module>`), kept even when unsupported by a realm.
    pub extensions: BTreeMap<String, Extension>,
}

fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(PortableError::Invalid(msg.into()))
}

fn too_many<T>(what: &str, max: usize) -> Result<T> {
    Err(PortableError::LimitExceeded(format!(
        "more than {max} {what}"
    )))
}

fn check_text(what: &str, s: &str, max_bytes: usize) -> Result<()> {
    if s.len() > max_bytes {
        return Err(PortableError::LimitExceeded(format!(
            "{what} is longer than {max_bytes} bytes"
        )));
    }
    Ok(())
}

fn check_name(what: &str, s: &str, max_chars: usize) -> Result<()> {
    let chars = s.chars().count();
    if chars == 0 || chars > max_chars {
        return invalid(format!("{what} must have 1 to {max_chars} characters"));
    }
    if s.chars().any(|c| c.is_control()) || s.trim() != s {
        return invalid(format!(
            "{what} contains control characters or surrounding spaces"
        ));
    }
    Ok(())
}

fn ascending_unique<T: Ord + Copy>(what: &str, ids: impl Iterator<Item = T>) -> Result<()> {
    let mut previous: Option<T> = None;
    for id in ids {
        if previous.is_some_and(|p| p >= id) {
            return invalid(format!(
                "{what} must be strictly ascending (call normalize first)"
            ));
        }
        previous = Some(id);
    }
    Ok(())
}

impl PortableCharacter {
    /// Canonical order: everything that is a set is sorted and de-duplicated, so equal characters serialise to equal
    /// bytes (and therefore to equal hashes).
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }

    pub fn normalize(&mut self) {
        fn merge_masks(list: &mut Vec<(u32, u8)>) {
            list.sort_by_key(|e| e.0);
            let mut out: Vec<(u32, u8)> = Vec::with_capacity(list.len());
            for (id, mask) in list.drain(..) {
                match out.last_mut() {
                    Some(last) if last.0 == id => last.1 |= mask,
                    _ => out.push((id, mask)),
                }
            }
            *list = out;
        }
        merge_masks(&mut self.build.spells);
        merge_masks(&mut self.build.talents);
        self.build.skills.sort_by_key(|s| s.skill);
        self.build.skills.dedup_by_key(|s| s.skill);
        self.build.glyphs.sort_by_key(|g| g.talent_group);
        self.build.glyphs.dedup_by_key(|g| g.talent_group);
        self.items.sort_by(|a, b| {
            let key = |i: &PortableItem| {
                (
                    i.container.is_some(),
                    i.container.map(|c| c.as_uuid()),
                    i.slot,
                    i.id.as_uuid(),
                )
            };
            key(a).cmp(&key(b))
        });
        for item in &mut self.items {
            item.enchantments.sort_by_key(|e| e.slot);
        }
        self.quests.active.sort_by_key(|q| q.quest);
        self.quests.rewarded.sort_unstable();
        self.quests.rewarded.dedup();
        self.reputation.sort_by_key(|r| r.faction);
        self.actions.sort_by_key(|a| (a.spec, a.button));
        self.pets.sort_by_key(|p| (p.slot, p.id.as_uuid()));
        for pet in &mut self.pets {
            pet.spells.sort_by_key(|s| s.spell);
        }
    }

    /// Structural validation. Run on everything that enters the store or comes from outside.
    pub fn validate(&self) -> Result<()> {
        if self.format_version != PORTABLE_CHARACTER_FORMAT_VERSION {
            return Err(PortableError::UnsupportedFormat {
                found: self.format_version,
                supported: PORTABLE_CHARACTER_FORMAT_VERSION,
            });
        }
        if !valid_namespace(&self.content_namespace) {
            return invalid("content_namespace is not a valid namespace");
        }

        // identity
        check_name("character name", &self.identity.name, MAX_NAME_CHARS)?;
        if self.identity.gender > 2 {
            return invalid("gender must be 0, 1 or 2");
        }

        // progression
        let p = &self.progression;
        if p.level == 0 {
            return invalid("level must be at least 1");
        }
        if p.money > MAX_MONEY {
            return invalid(format!(
                "money exceeds the realm limit of {MAX_MONEY} copper"
            ));
        }
        for (what, text) in [
            ("known_titles", &p.known_titles),
            ("explored_zones", &p.explored_zones),
            ("taxi_mask", &p.taxi_mask),
        ] {
            check_text(what, text, MAX_RAW_TEXT_BYTES)?;
            if !text.bytes().all(|b| b.is_ascii_digit() || b == b' ') {
                return invalid(format!("{what} may only contain digits and spaces"));
            }
        }

        // build
        let b = &self.build;
        if b.spells.len() > MAX_SPELLS {
            return too_many("spells", MAX_SPELLS);
        }
        if b.talents.len() > MAX_TALENTS {
            return too_many("talents", MAX_TALENTS);
        }
        if b.skills.len() > MAX_SKILLS {
            return too_many("skills", MAX_SKILLS);
        }
        if b.glyphs.len() > MAX_GLYPH_SETS {
            return too_many("glyph sets", MAX_GLYPH_SETS);
        }
        ascending_unique("spells", b.spells.iter().map(|e| e.0))?;
        ascending_unique("talents", b.talents.iter().map(|e| e.0))?;
        ascending_unique("skills", b.skills.iter().map(|e| e.skill))?;
        ascending_unique("glyph sets", b.glyphs.iter().map(|e| e.talent_group))?;

        self.validate_items()?;

        // quests, reputation, actions
        if self.quests.active.len() > MAX_ACTIVE_QUESTS {
            return too_many("active quests", MAX_ACTIVE_QUESTS);
        }
        if self.quests.rewarded.len() > MAX_REWARDED_QUESTS {
            return too_many("completed quests", MAX_REWARDED_QUESTS);
        }
        ascending_unique("active quests", self.quests.active.iter().map(|q| q.quest))?;
        ascending_unique("completed quests", self.quests.rewarded.iter().copied())?;
        if self.reputation.len() > MAX_REPUTATIONS {
            return too_many("reputations", MAX_REPUTATIONS);
        }
        ascending_unique("reputations", self.reputation.iter().map(|r| r.faction))?;
        if self.actions.len() > MAX_ACTIONS {
            return too_many("action buttons", MAX_ACTIONS);
        }
        ascending_unique(
            "action buttons",
            self.actions.iter().map(|a| (a.spec, a.button)),
        )?;

        // pets
        if self.pets.len() > MAX_PETS {
            return too_many("pets", MAX_PETS);
        }
        let mut pet_ids = HashSet::new();
        for pet in &self.pets {
            if !pet_ids.insert(pet.id) {
                return invalid("two pets have the same id");
            }
            check_name("pet name", &pet.name, MAX_PET_NAME_CHARS)?;
            check_text("pet action bar", &pet.action_bar, MAX_RAW_TEXT_BYTES)?;
            if pet.spells.len() > MAX_PET_SPELLS {
                return too_many("pet spells", MAX_PET_SPELLS);
            }
            ascending_unique("pet spells", pet.spells.iter().map(|s| s.spell))?;
            if let Some(names) = &pet.declined_names {
                for n in names {
                    check_text("pet declined name", n, 128)?;
                }
            }
        }

        // appearance
        if self.wardrobe.outfits.len() > MAX_APPEARANCE_OUTFITS {
            return too_many("saved outfits", MAX_APPEARANCE_OUTFITS);
        }
        if let Some(category) = self
            .wardrobe
            .active
            .keys()
            .find(|c| **c == 0 || **c > MAX_APPEARANCE_CATEGORY)
        {
            return invalid(format!("appearance category {category} is out of range"));
        }
        if self.wardrobe.active.values().any(|id| *id == 0) {
            return invalid(
                "an active appearance cannot be 0 (a category without a selection has no entry)",
            );
        }
        for (name, ids) in &self.wardrobe.outfits {
            if name.is_empty()
                || name.len() > MAX_APPEARANCE_OUTFIT_NAME_BYTES
                || name.chars().any(|c| (c as u32) < 0x20)
            {
                return invalid("an outfit name must have 1 to 64 bytes and no control characters");
            }
            if ids.len() > MAX_APPEARANCE_CATEGORY as usize + 1 {
                return too_many(
                    "appearances in one outfit",
                    MAX_APPEARANCE_CATEGORY as usize + 1,
                );
            }
        }

        // settings, client blobs, extensions
        if self.settings.len() > MAX_SETTINGS {
            return too_many("settings sources", MAX_SETTINGS);
        }
        for (source, values) in &self.settings {
            if source.is_empty()
                || source.len() > MAX_SETTING_SOURCE_BYTES
                || !source
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-:".contains(&c))
            {
                return invalid(format!("invalid settings source {source:?}"));
            }
            if values.len() > MAX_SETTING_VALUES {
                return too_many("values in one settings source", MAX_SETTING_VALUES);
            }
        }
        if self.client_data.len() > MAX_CLIENT_BLOBS {
            return too_many("client data blobs", MAX_CLIENT_BLOBS);
        }
        for blob in self.client_data.values() {
            if blob.data.0.len() > MAX_CLIENT_BLOB_BYTES {
                return Err(PortableError::LimitExceeded(format!(
                    "a client data blob is larger than {MAX_CLIENT_BLOB_BYTES} bytes"
                )));
            }
        }
        if self.extensions.len() > MAX_EXTENSIONS {
            return too_many("extensions", MAX_EXTENSIONS);
        }
        let mut total = 0usize;
        for (namespace, ext) in &self.extensions {
            if !valid_namespace(namespace) {
                return invalid(format!("invalid extension namespace {namespace:?}"));
            }
            if ext.format_version == 0
                || ext.module_version.is_empty()
                || ext.module_version.len() > 64
            {
                return invalid(format!("extension {namespace} has an invalid version"));
            }
            if ext.payload.0.len() > MAX_EXTENSION_BYTES {
                return Err(PortableError::LimitExceeded(format!(
                    "extension {namespace} is larger than {MAX_EXTENSION_BYTES} bytes"
                )));
            }
            if !ext.is_intact() {
                return Err(PortableError::CorruptSnapshot(format!(
                    "extension {namespace} does not match its content hash"
                )));
            }
            total += ext.payload.0.len();
        }
        if total > MAX_EXTENSIONS_TOTAL_BYTES {
            return Err(PortableError::LimitExceeded(format!(
                "extensions are larger than {MAX_EXTENSIONS_TOTAL_BYTES} bytes together"
            )));
        }
        Ok(())
    }

    fn validate_items(&self) -> Result<()> {
        if self.items.len() > MAX_ITEMS {
            return too_many("items", MAX_ITEMS);
        }
        let mut by_id: HashMap<PortableItemId, &PortableItem> =
            HashMap::with_capacity(self.items.len());
        for item in &self.items {
            if by_id.insert(item.id, item).is_some() {
                return invalid("two items have the same id");
            }
        }
        let mut places: BTreeSet<(Option<PortableItemId>, u8)> = BTreeSet::new();
        for item in &self.items {
            if item.slot > MAX_SLOT {
                return invalid(format!("item slot {} is out of range", item.slot));
            }
            if item.count == 0 {
                return invalid("an item stack cannot be empty");
            }
            if item.charges.len() > MAX_CHARGES {
                return too_many("charges on one item", MAX_CHARGES);
            }
            if let Some(text) = &item.text {
                check_text("item text", text, MAX_ITEM_TEXT_BYTES)?;
            }
            if let Some(name) = &item.creator_name {
                check_name("creator name", name, MAX_NAME_CHARS)?;
            }
            let mut last_slot: Option<u8> = None;
            for e in &item.enchantments {
                if e.slot >= MAX_ENCHANT_SLOTS || last_slot.is_some_and(|l| l >= e.slot) {
                    return invalid("enchantment slots must be ascending and below 12");
                }
                last_slot = Some(e.slot);
            }
            if let Some(container) = item.container {
                match by_id.get(&container) {
                    None => {
                        return invalid(
                            "an item is in a container that is not part of the character",
                        )
                    }
                    Some(c) if c.container.is_some() => {
                        return invalid("a container must be directly on the character")
                    }
                    Some(_) => {}
                }
            }
            if !places.insert((item.container, item.slot)) {
                return invalid("two items occupy the same place");
            }
        }
        Ok(())
    }
}
