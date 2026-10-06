//! Realm rows -> portable character. A pure function of one realm answer (`RawExport`): no database access here, so
//! it is tested against recorded answers of a real database.
//!
//! Nothing is copied blindly: every field is read by name, converted to the model's types, and the model is validated
//! (limits, container tree, ...) before it is returned. Items the realm itself would delete on load (orphans) are left
//! out with a warning instead of being exported.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::super::error::{PortableError, Result};
use super::super::identity::item_identity;
use super::super::ids::{CharacterId, ContentId, PortableItemId, PortablePetId};
use super::super::model::*;
use super::super::store::ItemObservation;
use super::super::versions::PORTABLE_CHARACTER_FORMAT_VERSION;
use super::blockers::{is_internal_account, Blocker};
use super::policy::{classify_setting, Disposition, QUARANTINE_EXTENSION, SETTINGS_POLICY_VERSION};
use super::script::{RawExport, Row};

/// Only the cosmetic bits of `playerFlags` are carried: hide helm (0x400) and hide cloak (0x800).
pub const COSMETIC_FLAG_MASK: u32 = 0x0C00;
/// Buyback slots are never persisted by the realm.
const BUYBACK_SLOTS: std::ops::RangeInclusive<u8> = 74..=85;

pub struct ExportRequest<'a> {
    pub ruleset: Ruleset,
    pub local_guid: u32,
    /// `None` = a new portable character (new UUIDv7); `Some` = a re-export of an existing one.
    pub character_id: Option<CharacterId>,
    /// Active item mappings of that character on this realm: local item guid -> (portable item id, identity).
    pub prior_items: &'a HashMap<u32, (PortableItemId, String)>,
}

#[derive(Debug)]
pub struct Exported {
    pub model: PortableCharacter,
    pub local_guid: u32,
    /// The realm account that owns the character (never carried; used to describe the source).
    pub account: u32,
    /// One entry per exported item, ready for `Store::reconcile_item_mappings`.
    pub observations: Vec<ItemObservation>,
    /// Things that were left out or looked wrong; shown to the user, never silently dropped.
    pub warnings: Vec<String>,
}

fn corrupt<T>(msg: impl Into<String>) -> Result<T> {
    Err(PortableError::CorruptSnapshot(msg.into()))
}

/// Why this character cannot be exported now. Empty = it can.
pub fn blockers(raw: &RawExport) -> Result<Vec<Blocker>> {
    let chars = raw.section("chars")?;
    let row = chars.iter().next().ok_or_else(|| PortableError::NoSuchRealmCharacter(0))?;
    let mut out = Vec::new();
    if row.u64("online")? != 0 {
        out.push(Blocker::Online);
    }
    if row.u64("deleted")? != 0 {
        out.push(Blocker::Deleted);
    }
    if row.opt_text("username")?.is_some_and(|u| is_internal_account(&u)) {
        out.push(Blocker::BotAccount);
    }
    for (section, blocker) in [
        ("block:challenge", Blocker::ActiveChallenge),
        ("block:gamemode", Blocker::ActiveGameMode),
        ("block:trial", Blocker::ActiveCustomTrial),
        ("block:manastorm", Blocker::PendingManastormCaches),
    ] {
        if raw.has(section) && raw.section(section)?.iter().next().map(|r| r.u64("n")).transpose()?.unwrap_or(0) > 0 {
            out.push(blocker);
        }
    }
    for name in raw.names().filter(|n| n.starts_with("unclassified:")) {
        let n = raw.section(name)?.iter().next().map(|r| r.u64("n")).transpose()?.unwrap_or(0);
        if n > 0 {
            out.push(Blocker::UnclassifiedState(name["unclassified:".len()..].to_string()));
        }
    }
    out.sort_by_key(|b| b.code());
    Ok(out)
}

pub fn build(raw: &RawExport, req: &ExportRequest<'_>) -> Result<Exported> {
    let chars = raw.section("chars")?;
    let c = chars.iter().next().ok_or(PortableError::NoSuchRealmCharacter(req.local_guid))?;
    if c.u32("guid")? != req.local_guid {
        return corrupt("the realm answered for another character");
    }
    let blocked = blockers(raw)?;
    if !blocked.is_empty() {
        return Err(PortableError::NotExportable(blocked));
    }

    let ns = req.ruleset.as_str();
    let content = |kind: &str, id: u64| ContentId::new(ns, kind, id);
    let mut warnings = Vec::new();

    let identity = Identity {
        name: c.text("name")?,
        race: content("race", c.u64("race")?)?,
        class: content("class", c.u64("class")?)?,
        gender: c.u8("gender")?,
        appearance: Appearance {
            skin: c.u8("skin")?,
            face: c.u8("face")?,
            hair_style: c.u8("hair_style")?,
            hair_color: c.u8("hair_color")?,
            facial_style: c.u8("facial_style")?,
        },
        cosmetic_flags: c.u32("player_flags")? & COSMETIC_FLAG_MASK,
    };

    let progression = Progression {
        level: c.u8("level")?,
        xp: c.u32("xp")?,
        money: c.u32("money")?,
        honor: Honor {
            arena_points: c.u32("arena_points")?,
            total_honor: c.u32("total_honor")?,
            today_honor: c.u32("today_honor")?,
            yesterday_honor: c.u32("yesterday_honor")?,
            total_kills: c.u32("total_kills")?,
            today_kills: c.u16("today_kills")?,
            yesterday_kills: c.u16("yesterday_kills")?,
        },
        known_currencies: c.u64("known_currencies")?,
        chosen_title: c.u32("chosen_title")?,
        known_titles: c.text("known_titles")?,
        explored_zones: c.text("explored_zones")?,
        taxi_mask: c.text("taxi_mask")?,
        bank_slots: c.u8("bank_slots")?,
        stable_slots: c.u8("stable_slots")?,
        watched_faction: c.u32("watched_faction")?,
        action_bars_mask: c.u8("action_bars")?,
    };

    let build = Build {
        spells: pairs(raw, "spells")?,
        talents: pairs(raw, "talents")?,
        skills: raw.section("skills")?.iter().map(|r| Ok(Skill { skill: r.u32("skill")?, value: r.u16("value")?, max: r.u16("max")? })).collect::<Result<_>>()?,
        glyphs: raw
            .section("glyphs")?
            .iter()
            .map(|r| Ok(GlyphSet { talent_group: r.u8("talent_group")?, glyphs: [r.u16("g1")?, r.u16("g2")?, r.u16("g3")?, r.u16("g4")?, r.u16("g5")?, r.u16("g6")?] }))
            .collect::<Result<_>>()?,
        talent_groups_count: c.u8("talent_groups")?,
        active_talent_group: c.u8("active_group")?,
        extra_bonus_talent_count: c.i32("extra_talents")?,
        reset_talents_cost: c.u32("reset_cost")?,
    };

    let (items, observations) = items(raw, req, ns, &mut warnings)?;

    let quests = Quests {
        active: raw
            .section("quests")?
            .iter()
            .map(|r| {
                Ok(ActiveQuest {
                    quest: r.u32("quest")?,
                    status: r.u8("status")?,
                    explored: r.u64("explored")? != 0,
                    timer: r.u32("timer")?,
                    mob_counts: [r.u16("mob1")?, r.u16("mob2")?, r.u16("mob3")?, r.u16("mob4")?],
                    item_counts: [r.u16("item1")?, r.u16("item2")?, r.u16("item3")?, r.u16("item4")?, r.u16("item5")?, r.u16("item6")?],
                    player_count: r.u16("player_count")?,
                })
            })
            .collect::<Result<_>>()?,
        rewarded: raw.section("rewarded")?.iter().map(|r| r.u32("quest")).collect::<Result<_>>()?,
    };

    let reputation = raw
        .section("reputation")?
        .iter()
        .map(|r| Ok(ReputationEntry { faction: r.u32("faction")?, standing: r.i32("standing")?, flags: r.u32("flags")? }))
        .collect::<Result<_>>()?;
    let actions = raw
        .section("actions")?
        .iter()
        .map(|r| Ok(ActionButton { spec: r.u8("spec")?, button: r.u8("button")?, action: r.u32("action")?, kind: r.u8("type")? }))
        .collect::<Result<_>>()?;

    let pets = pets(raw, ns)?;
    let (settings, quarantined) = settings(raw, req.ruleset)?;

    let mut client_data = BTreeMap::new();
    if let Some(r) = raw.section("macros")?.iter().next() {
        client_data.insert(5u8, ClientBlob { time: r.u32("time")?, data: Bytes(r.bytes("data")?) });
    }

    let mut extensions = BTreeMap::new();
    if !quarantined.is_empty() {
        // BTreeMap -> deterministic bytes, so an unchanged quarantine has an unchanged hash
        let payload = serde_json::to_vec(&quarantined)?;
        extensions.insert(QUARANTINE_EXTENSION.to_string(), Extension::new(&SETTINGS_POLICY_VERSION.to_string(), 1, payload));
        warnings.push(format!("{} settings source(s) are not known gameplay state and were kept aside, not carried: {}", quarantined.len(), quarantined.keys().cloned().collect::<Vec<_>>().join(", ")));
    }

    let model = PortableCharacter {
        format_version: PORTABLE_CHARACTER_FORMAT_VERSION,
        character_id: req.character_id.unwrap_or_default(),
        ruleset: req.ruleset,
        content_namespace: ns.to_string(),
        identity,
        progression,
        build,
        items,
        quests,
        reputation,
        actions,
        pets,
        settings,
        client_data,
        extensions,
    }
    .normalized();
    model.validate()?;

    Ok(Exported { model, local_guid: req.local_guid, account: c.u32("account")?, observations, warnings })
}

fn pairs(raw: &RawExport, section: &str) -> Result<Vec<(u32, u8)>> {
    raw.section(section)?.iter().map(|r| Ok((r.u32("spell")?, r.u8("spec_mask")?))).collect()
}

fn numbers(text: &str, what: &str) -> Result<Vec<i64>> {
    text.split_whitespace()
        .map(|t| t.parse::<i64>().map_err(|_| PortableError::CorruptSnapshot(format!("{what}: {t:?} is not a number"))))
        .collect()
}

fn items(raw: &RawExport, req: &ExportRequest<'_>, ns: &str, warnings: &mut Vec<String>) -> Result<(Vec<PortableItem>, Vec<ItemObservation>)> {
    let instances: HashMap<u32, Row<'_>> = raw.section("items")?.iter().map(|r| Ok((r.u32("guid")?, r))).collect::<Result<_>>()?;
    let gifts: HashMap<u32, Row<'_>> = raw.section("gifts")?.iter().map(|r| Ok((r.u32("item_guid")?, r))).collect::<Result<_>>()?;

    // First pass: which inventory rows are usable at all.
    struct Placed {
        guid: u32,
        bag: u32,
        slot: u8,
    }
    let mut placed = Vec::new();
    for r in raw.section("inventory")?.iter() {
        let (bag, slot, guid) = (r.u32("bag")?, r.u64("slot")?, r.u32("item")?);
        let Ok(slot) = u8::try_from(slot) else {
            warnings.push(format!("item {guid}: slot {slot} is out of range, left out"));
            continue;
        };
        if !instances.contains_key(&guid) {
            warnings.push(format!("item {guid}: listed in the inventory but missing from item_instance (the realm would delete it), left out"));
            continue;
        }
        if bag == 0 && BUYBACK_SLOTS.contains(&slot) {
            warnings.push(format!("item {guid}: in a buyback slot, left out"));
            continue;
        }
        placed.push(Placed { guid, bag, slot });
    }
    let on_character: HashSet<u32> = placed.iter().filter(|p| p.bag == 0).map(|p| p.guid).collect();

    let mut ids: HashMap<u32, PortableItemId> = HashMap::new();
    let mut observations = Vec::new();
    let mut items = Vec::new();
    for p in &placed {
        if p.bag != 0 && !on_character.contains(&p.bag) {
            warnings.push(format!("item {}: its container {} is not on the character (the realm would delete it), left out", p.guid, p.bag));
            continue;
        }
        let r = instances[&p.guid];
        let entry = ContentId::new(ns, "item", r.u64("entry")?)?;
        let random_property_id = r.i32("random_property")?;
        let creator_name = if r.u64("creator_guid")? != 0 { r.opt_text("creator_name")?.filter(|n| !n.is_empty()) } else { None };
        let identity = item_identity(&entry, random_property_id);

        // keep the portable id when this local guid is still the same item; a recycled guid gets a new one
        let id = match req.prior_items.get(&p.guid) {
            Some((id, prior_identity)) if *prior_identity == identity => *id,
            _ => PortableItemId::new(),
        };
        ids.insert(p.guid, id);

        let enchantment_numbers = numbers(&r.text("enchantments")?, "enchantments")?;
        if enchantment_numbers.len() % 3 != 0 || enchantment_numbers.len() > 36 {
            warnings.push(format!("item {}: unusual enchantment data ({} values)", p.guid, enchantment_numbers.len()));
        }
        let mut enchantments = Vec::new();
        for (slot, chunk) in enchantment_numbers.chunks_exact(3).take(12).enumerate() {
            if chunk.iter().any(|v| *v != 0) {
                let to = |v: i64| u32::try_from(v).map_err(|_| PortableError::CorruptSnapshot(format!("item {}: enchantment value {v} is out of range", p.guid)));
                enchantments.push(Enchantment { slot: slot as u8, id: to(chunk[0])?, duration: to(chunk[1])?, charges: to(chunk[2])? });
            }
        }
        let charges: Vec<i32> = match r.opt_text("charges")? {
            Some(text) => numbers(&text, "charges")?.into_iter().take(5).map(|v| i32::try_from(v).map_err(|_| PortableError::CorruptSnapshot("item charges out of range".into()))).collect::<Result<_>>()?,
            None => Vec::new(),
        };
        let gift = match gifts.get(&p.guid) {
            Some(g) => Some(Gift { entry: ContentId::new(ns, "item", g.u64("entry")?)?, flags: g.u32("flags")? }),
            None => None,
        };

        observations.push(ItemObservation { portable_item_id: id, local_item_guid: p.guid, entry: entry.clone(), identity });
        items.push((
            p.bag,
            PortableItem {
                id,
                container: None,
                slot: p.slot,
                entry,
                count: r.u32("count")?,
                duration: r.i32("duration")?,
                charges,
                flags: r.u32("flags")?,
                enchantments,
                random_property_id,
                durability: r.u32("durability")?,
                played_time: r.u32("played_time")?,
                text: r.opt_text("text")?.filter(|t| !t.is_empty()),
                creator_name,
                gift,
            },
        ));
    }
    // containers are resolved after every id exists
    let items = items
        .into_iter()
        .map(|(bag, mut item)| {
            if bag != 0 {
                item.container = ids.get(&bag).copied();
            }
            item
        })
        .collect();
    Ok((items, observations))
}

fn pets(raw: &RawExport, ns: &str) -> Result<Vec<PortablePet>> {
    let mut spells: HashMap<u32, Vec<PetSpell>> = HashMap::new();
    for r in raw.section("pet_spells")?.iter() {
        spells.entry(r.u32("pet")?).or_default().push(PetSpell { spell: r.u32("spell")?, active: r.u8("active")? });
    }
    let mut declined: HashMap<u32, [String; 5]> = HashMap::new();
    for r in raw.section("pet_declined")?.iter() {
        declined.insert(r.u32("id")?, [r.text("n1")?, r.text("n2")?, r.text("n3")?, r.text("n4")?, r.text("n5")?]);
    }
    raw.section("pets")?
        .iter()
        .map(|r| {
            let number = r.u32("id")?;
            Ok(PortablePet {
                id: PortablePetId::new(),
                entry: ContentId::new(ns, "creature", r.u64("entry")?)?,
                model_id: r.u32("model")?,
                created_by_spell: r.u32("created_by")?,
                pet_type: r.u8("pet_type")?,
                level: r.u16("level")?,
                exp: r.u32("exp")?,
                react_state: r.u8("react")?,
                name: r.text("name")?,
                renamed: r.u64("renamed")? != 0,
                slot: r.u8("slot")?,
                health: r.u32("health")?,
                mana: r.u32("mana")?,
                happiness: r.u32("happiness")?,
                action_bar: r.text("abdata")?,
                spells: spells.remove(&number).unwrap_or_default(),
                declined_names: declined.remove(&number),
            })
        })
        .collect()
}

/// `(carried, quarantined)`.
fn settings(raw: &RawExport, ruleset: Ruleset) -> Result<(BTreeMap<String, Vec<u32>>, BTreeMap<String, Vec<u32>>)> {
    let mut carried = BTreeMap::new();
    let mut quarantined = BTreeMap::new();
    for r in raw.section("settings")?.iter() {
        let source = r.text("source")?;
        let values: Vec<u32> = r
            .text("data")?
            .split_whitespace()
            .map(|t| t.parse::<u32>().map_err(|_| PortableError::CorruptSnapshot(format!("settings {source}: {t:?} is not an unsigned integer"))))
            .collect::<Result<_>>()?;
        match classify_setting(&source, ruleset) {
            Disposition::Carry => {
                carried.insert(source, values);
            }
            Disposition::Quarantine => {
                quarantined.insert(source, values);
            }
            Disposition::Drop => {}
        }
    }
    Ok((carried, quarantined))
}
