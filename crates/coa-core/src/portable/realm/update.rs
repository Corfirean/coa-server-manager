//! Phase 4: bringing an **existing** realm character to a newer canonical state, in place.
//!
//! `build_update(current, merged, ctx)` turns the difference between what the realm holds *now* (`current`, a fresh export)
//! and what it should hold afterwards (`merged`, the three-way merge of the realm's state, the snapshot it was last synced
//! with, and the new canonical) into one fixed SQL script that runs in one MySQL transaction.
//!
//! What this never does:
//! * delete or re-create the character: the local guid, the account, `online`, `logout_time`, the **position** (`map`,
//!   `zone`, `position_*`, `orientation`, ...) and the **homebind** are not touched;
//! * write a column that did not change: the script is a *delta* of the portable subset, so everything the model does not
//!   carry (world-local data) stays exactly as the realm has it;
//! * touch an item, pet or row that is not in the model: items and pets are addressed by their mapped local ids and only
//!   rows the diff names are changed.
//!
//! Layout: lock -> guards (realm stopped, the character is what was read) -> allocation -> deletes -> updates -> inserts ->
//! assertions -> marker -> `COMMIT` -> report. Untrusted text and blobs are hex literals from the typed encoder, as in the
//! importer.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::super::error::{PortableError, Result};
use super::super::ids::{PortableItemId, PortablePetId};
use super::super::model::*;
use super::plan::{charges_text, content_id, enchantment_text, settings_text, FAIL, IMPORT_LOCK, ITEM_KEYED, PET_KEYED};
use super::policy::{classify_setting, Disposition, ALLOC_MARKER_SOURCE, IMPORT_MARKER_SOURCE};
use super::script::SchemaProbe;
use super::sqlenc::{Delete, Insert, Update, Val};

pub struct UpdateContext<'a> {
    pub ruleset: Ruleset,
    pub local_guid: u32,
    /// The canonical revision the realm is brought to.
    pub revision: u64,
    pub nonce: [u32; 4],
    pub game_server_users: &'a [String],
    pub probe: &'a SchemaProbe,
    /// The items the realm holds now (portable id -> local guid): what `current` was exported from.
    pub items: &'a HashMap<PortableItemId, u32>,
    pub pets: &'a HashMap<PortablePetId, u32>,
}

/// What the script does, in numbers (and as the plan the journal records).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UpdateCounts {
    pub character_columns: usize,
    pub items_added: usize,
    pub items_removed: usize,
    pub items_changed: usize,
    pub items_moved: usize,
    pub pets_added: usize,
    pub pets_removed: usize,
    pub pets_changed: usize,
    /// Rows inserted or deleted in keyed tables (spells, skills, reputation, quests, actions, glyphs, settings, macros).
    pub keyed_rows: usize,
}

impl UpdateCounts {
    pub fn is_empty(&self) -> bool {
        *self == UpdateCounts::default()
    }
}

#[derive(Debug)]
pub struct UpdateScript {
    pub script: String,
    /// Item `i` of this list is realm guid `@item_base + i`.
    pub added_items: Vec<PortableItemId>,
    pub added_pets: Vec<PortablePetId>,
    pub removed_items: Vec<PortableItemId>,
    pub removed_pets: Vec<PortablePetId>,
    pub counts: UpdateCounts,
    pub marker_data: String,
}

fn diff<K: Ord + Clone, V: PartialEq + Clone>(current: &BTreeMap<K, V>, new: &BTreeMap<K, V>) -> (Vec<K>, Vec<(K, V)>) {
    let (mut delete, mut insert) = (Vec::new(), Vec::new());
    for (k, v) in current {
        match new.get(k) {
            None => delete.push(k.clone()),
            Some(n) if n != v => {
                delete.push(k.clone());
                insert.push((k.clone(), n.clone()));
            }
            Some(_) => {}
        }
    }
    for (k, n) in new {
        if !current.contains_key(k) {
            insert.push((k.clone(), n.clone()));
        }
    }
    (delete, insert)
}

/// Inventory rows of `@char` whose item instance does not exist.
const ORPHAN_ITEMS: &str = "(SELECT COUNT(*) FROM acore_characters.character_inventory ci LEFT JOIN acore_characters.item_instance ii ON ii.guid = ci.item AND ii.owner_guid = @char WHERE ci.guid = @char AND ii.guid IS NULL)";
/// Inventory rows of `@char` that sit in a bag the character does not have.
const BAGLESS_ITEMS: &str = "(SELECT COUNT(*) FROM acore_characters.character_inventory ci LEFT JOIN acore_characters.character_inventory b ON b.item = ci.bag AND b.guid = @char WHERE ci.guid = @char AND ci.bag <> 0 AND b.item IS NULL)";

fn max_of(probe: &SchemaProbe, list: &[(&str, &str)]) -> String {
    let parts: Vec<String> = list.iter().filter(|(t, _)| probe.has(t)).map(|(t, c)| format!("IFNULL((SELECT MAX(`{c}`) FROM acore_characters.`{t}`), 0)")).collect();
    format!("GREATEST(0, {})", parts.join(", "))
}

/// Statements in the order they must run: every delete, then every update, then every insert.
#[derive(Default)]
struct Statements {
    guards: Vec<String>,
    deletes: Vec<String>,
    updates: Vec<String>,
    inserts: Vec<String>,
}

impl Statements {
    fn delete(&mut self, d: Delete) {
        self.deletes.extend(d.sql());
    }
    fn update(&mut self, u: Update) {
        self.updates.extend(u.sql());
    }
    fn insert(&mut self, i: Insert) {
        self.inserts.extend(i.sql());
    }
}

const COSMETIC_KEEP: [(u32, &str); 4] = [
    (0, "(`playerFlags` & ~3072)"),
    (0x400, "((`playerFlags` & ~3072) | 1024)"),
    (0x800, "((`playerFlags` & ~3072) | 2048)"),
    (0xC00, "((`playerFlags` & ~3072) | 3072)"),
];

fn cannot(what: &str) -> PortableError {
    PortableError::Invalid(format!("{what} cannot be changed in place; the character would have to be imported again"))
}

pub fn build_update(current: &PortableCharacter, merged: &PortableCharacter, ctx: &UpdateContext<'_>) -> Result<UpdateScript> {
    merged.validate()?;
    if current.ruleset != ctx.ruleset || merged.ruleset != ctx.ruleset {
        return Err(PortableError::WrongRealm { expected: merged.ruleset.to_string(), found: ctx.ruleset.to_string() });
    }
    if ctx.game_server_users.is_empty() {
        return Err(PortableError::Invalid("internal: no game server user is named, the realm cannot be proven stopped".into()));
    }
    let ns = ctx.ruleset.as_str();
    let (ci, mi) = (&current.identity, &merged.identity);
    if ci.race != mi.race || ci.class != mi.class || ci.gender != mi.gender {
        return Err(cannot("race, class and gender"));
    }

    let mut st = Statements::default();
    let mut counts = UpdateCounts::default();
    let char_key = || vec![("guid", Val::Expr("@char"))];

    // ---- the characters row: only the columns that differ ---------------------------------------------------------
    let mut row = Update::new("characters", char_key());
    if ci.name != mi.name {
        row.set("name", Val::text(mi.name.clone()));
        let name = Val::text(mi.name.clone()).sql();
        let collate = format!("name COLLATE utf8mb4_unicode_ci = {name} COLLATE utf8mb4_unicode_ci");
        st.guards.push(format!("DO IF(NOT EXISTS(SELECT 1 FROM acore_characters.characters WHERE guid <> @char AND {collate}), 1, {FAIL});"));
        if ctx.probe.has("reserved_name") {
            st.guards.push(format!("DO IF(NOT EXISTS(SELECT 1 FROM acore_characters.reserved_name WHERE {collate}), 1, {FAIL});"));
        }
    }
    let (a, b) = (&ci.appearance, &mi.appearance);
    for (column, old, new) in [("skin", a.skin, b.skin), ("face", a.face, b.face), ("hairStyle", a.hair_style, b.hair_style), ("hairColor", a.hair_color, b.hair_color), ("facialStyle", a.facial_style, b.facial_style)] {
        if old != new {
            row.set(column, Val::u(new));
        }
    }
    if ci.cosmetic_flags != mi.cosmetic_flags {
        let expr = COSMETIC_KEEP.iter().find(|(v, _)| *v == mi.cosmetic_flags).map(|(_, e)| *e).ok_or_else(|| PortableError::Invalid(format!("cosmetic flags {:#x} are not carried", mi.cosmetic_flags)))?;
        row.set("playerFlags", Val::Expr(expr));
    }
    let (p, q) = (&current.progression, &merged.progression);
    let (ph, qh) = (&p.honor, &q.honor);
    macro_rules! num {
        ($($col:literal: $old:expr => $new:expr),* $(,)?) => { $( if $old != $new { row.set($col, Val::u($new)); } )* };
    }
    num! {
        "level": p.level => q.level, "xp": p.xp => q.xp, "money": p.money => q.money,
        "arenaPoints": ph.arena_points => qh.arena_points, "totalHonorPoints": ph.total_honor => qh.total_honor,
        "todayHonorPoints": ph.today_honor => qh.today_honor, "yesterdayHonorPoints": ph.yesterday_honor => qh.yesterday_honor,
        "totalKills": ph.total_kills => qh.total_kills, "todayKills": ph.today_kills => qh.today_kills, "yesterdayKills": ph.yesterday_kills => qh.yesterday_kills,
        "knownCurrencies": p.known_currencies => q.known_currencies, "chosenTitle": p.chosen_title => q.chosen_title,
        "bankSlots": p.bank_slots => q.bank_slots, "stable_slots": p.stable_slots => q.stable_slots,
        "watchedFaction": p.watched_faction => q.watched_faction, "actionBars": p.action_bars_mask => q.action_bars_mask,
        "talentGroupsCount": current.build.talent_groups_count => merged.build.talent_groups_count,
        "activeTalentGroup": current.build.active_talent_group => merged.build.active_talent_group,
        "resettalents_cost": current.build.reset_talents_cost => merged.build.reset_talents_cost,
    }
    for (column, old, new) in [("knownTitles", &p.known_titles, &q.known_titles), ("exploredZones", &p.explored_zones, &q.explored_zones), ("taximask", &p.taxi_mask, &q.taxi_mask)] {
        if old != new {
            row.set(column, Val::text(new.clone()));
        }
    }
    if current.build.extra_bonus_talent_count != merged.build.extra_bonus_talent_count {
        row.set("extraBonusTalentCount", Val::i(merged.build.extra_bonus_talent_count));
    }
    counts.character_columns = row.len();
    st.update(row);

    // ---- keyed tables -----------------------------------------------------------------------------------------------
    let key_char = |extra: Vec<(&'static str, Val)>| -> Vec<(&'static str, Val)> {
        let mut k = char_key();
        k.extend(extra);
        k
    };

    // spells
    let spells = |m: &PortableCharacter| -> BTreeMap<u32, u8> { m.build.spells.iter().copied().collect() };
    let (del, ins) = diff(&spells(current), &spells(merged));
    counts.keyed_rows += del.len() + ins.len();
    st.delete(Delete::any_of("character_spell", char_key(), "spell", del.iter().map(|s| Val::u(*s)).collect()));
    let mut t = Insert::new("character_spell", &["guid", "spell", "specMask"]);
    for (spell, mask) in ins {
        t.row(vec![Val::Expr("@char"), Val::u(spell), Val::u(mask)])?;
    }
    st.insert(t);

    // skills
    let skills = |m: &PortableCharacter| -> BTreeMap<u32, (u16, u16)> { m.build.skills.iter().map(|s| (s.skill, (s.value, s.max))).collect() };
    let (del, ins) = diff(&skills(current), &skills(merged));
    counts.keyed_rows += del.len() + ins.len();
    st.delete(Delete::any_of("character_skills", char_key(), "skill", del.iter().map(|s| Val::u(*s)).collect()));
    let mut t = Insert::new("character_skills", &["guid", "skill", "value", "max"]);
    for (skill, (value, max)) in ins {
        t.row(vec![Val::Expr("@char"), Val::u(skill), Val::u(value), Val::u(max)])?;
    }
    st.insert(t);

    // glyphs
    let glyphs = |m: &PortableCharacter| -> BTreeMap<u8, [u16; 6]> { m.build.glyphs.iter().map(|g| (g.talent_group, g.glyphs)).collect() };
    let (del, ins) = diff(&glyphs(current), &glyphs(merged));
    counts.keyed_rows += del.len() + ins.len();
    st.delete(Delete::any_of("character_glyphs", char_key(), "talentGroup", del.iter().map(|g| Val::u(*g)).collect()));
    let mut t = Insert::new("character_glyphs", &["guid", "talentGroup", "glyph1", "glyph2", "glyph3", "glyph4", "glyph5", "glyph6"]);
    for (group, set) in ins {
        let mut r = vec![Val::Expr("@char"), Val::u(group)];
        r.extend(set.iter().map(|v| Val::u(*v)));
        t.row(r)?;
    }
    st.insert(t);

    // reputation
    let rep = |m: &PortableCharacter| -> BTreeMap<u32, (i32, u32)> { m.reputation.iter().map(|r| (r.faction, (r.standing, r.flags))).collect() };
    let (del, ins) = diff(&rep(current), &rep(merged));
    counts.keyed_rows += del.len() + ins.len();
    st.delete(Delete::any_of("character_reputation", char_key(), "faction", del.iter().map(|f| Val::u(*f)).collect()));
    let mut t = Insert::new("character_reputation", &["guid", "faction", "standing", "flags"]);
    for (faction, (standing, flags)) in ins {
        t.row(vec![Val::Expr("@char"), Val::u(faction), Val::i(standing), Val::u(flags)])?;
    }
    st.insert(t);

    // quests in progress, quests rewarded
    let quests = |m: &PortableCharacter| -> BTreeMap<u32, ActiveQuest> { m.quests.active.iter().map(|q| (q.quest, q.clone())).collect() };
    let (del, ins) = diff(&quests(current), &quests(merged));
    counts.keyed_rows += del.len() + ins.len();
    st.delete(Delete::any_of("character_queststatus", char_key(), "quest", del.iter().map(|q| Val::u(*q)).collect()));
    let mut t = Insert::new(
        "character_queststatus",
        &["guid", "quest", "status", "explored", "timer", "mobcount1", "mobcount2", "mobcount3", "mobcount4", "itemcount1", "itemcount2", "itemcount3", "itemcount4", "itemcount5", "itemcount6", "playercount"],
    );
    for (_, q) in ins {
        let mut r = vec![Val::Expr("@char"), Val::u(q.quest), Val::u(q.status), Val::u(q.explored as u8), Val::u(q.timer)];
        r.extend(q.mob_counts.iter().map(|v| Val::u(*v)));
        r.extend(q.item_counts.iter().map(|v| Val::u(*v)));
        r.push(Val::u(q.player_count));
        t.row(r)?;
    }
    st.insert(t);
    let rewarded = |m: &PortableCharacter| -> BTreeMap<u32, ()> { m.quests.rewarded.iter().map(|q| (*q, ())).collect() };
    let (del, ins) = diff(&rewarded(current), &rewarded(merged));
    counts.keyed_rows += del.len() + ins.len();
    st.delete(Delete::any_of("character_queststatus_rewarded", char_key(), "quest", del.iter().map(|q| Val::u(*q)).collect()));
    let mut t = Insert::new("character_queststatus_rewarded", &["guid", "quest", "active"]);
    for (quest, _) in ins {
        t.row(vec![Val::Expr("@char"), Val::u(quest), Val::u(1u8)])?;
    }
    st.insert(t);

    // action buttons (key: spec + button)
    let actions = |m: &PortableCharacter| -> BTreeMap<(u8, u8), (u32, u8)> { m.actions.iter().map(|a| ((a.spec, a.button), (a.action, a.kind))).collect() };
    let (del, ins) = diff(&actions(current), &actions(merged));
    counts.keyed_rows += del.len() + ins.len();
    for (spec, button) in del {
        st.delete(Delete::new("character_action", key_char(vec![("spec", Val::u(spec)), ("button", Val::u(button))])));
    }
    let mut t = Insert::new("character_action", &["guid", "spec", "button", "action", "type"]);
    for ((spec, button), (action, kind)) in ins {
        t.row(vec![Val::Expr("@char"), Val::u(spec), Val::u(button), Val::u(action), Val::u(kind)])?;
    }
    st.insert(t);

    // settings: only the strictly carried keys are ever written
    let carried = |m: &PortableCharacter| -> BTreeMap<String, Vec<u32>> { m.settings.iter().filter(|(k, _)| classify_setting(k, m.ruleset) == Disposition::Carry).map(|(k, v)| (k.clone(), v.clone())).collect() };
    let (del, ins) = diff(&carried(current), &carried(merged));
    counts.keyed_rows += del.len() + ins.len();
    st.delete(Delete::any_of("character_settings", char_key(), "source", del.iter().map(|s| Val::text(s.clone())).collect()));
    let mut t = Insert::new("character_settings", &["guid", "source", "data"]);
    for (source, values) in ins {
        t.row(vec![Val::Expr("@char"), Val::text(source), Val::text(settings_text(&values))])?;
    }
    st.insert(t);

    // the macros blob (account data type 5)
    if current.client_data.get(&5) != merged.client_data.get(&5) {
        counts.keyed_rows += 1;
        st.delete(Delete::new("character_account_data", key_char(vec![("type", Val::u(5u8))])));
        if let Some(blob) = merged.client_data.get(&5) {
            let mut t = Insert::new("character_account_data", &["guid", "type", "time", "data"]);
            t.row(vec![Val::Expr("@char"), Val::u(5u8), Val::u(blob.time), Val::Bytes(blob.data.0.clone())])?;
            st.insert(t);
        }
    }

    // ---- items ------------------------------------------------------------------------------------------------------
    let cur_items: HashMap<PortableItemId, &PortableItem> = current.items.iter().map(|i| (i.id, i)).collect();
    let new_items: HashMap<PortableItemId, &PortableItem> = merged.items.iter().map(|i| (i.id, i)).collect();
    for id in cur_items.keys() {
        if !ctx.items.contains_key(id) {
            return Err(PortableError::Invalid(format!("internal: item {id} of the realm has no local guid")));
        }
    }
    let removed_items: Vec<PortableItemId> = current.items.iter().map(|i| i.id).filter(|id| !new_items.contains_key(id)).collect();
    let added_items: Vec<PortableItemId> = merged.items.iter().map(|i| i.id).filter(|id| !cur_items.contains_key(id)).collect();
    let added_index: HashMap<PortableItemId, u32> = added_items.iter().enumerate().map(|(i, id)| (*id, i as u32)).collect();
    let local = |id: &PortableItemId| -> Result<Val> {
        match (ctx.items.get(id), added_index.get(id)) {
            (Some(g), _) if cur_items.contains_key(id) => Ok(Val::u(*g)),
            (_, Some(i)) => Ok(Val::ItemRef(*i)),
            _ => Err(PortableError::Invalid(format!("an item names the container {id}, which the character does not have"))),
        }
    };
    let bag_of = |item: &PortableItem| -> Result<Val> { item.container.as_ref().map_or(Ok(Val::u(0u8)), local) };

    let mut moved_inventory: Vec<PortableItemId> = Vec::new();
    let mut regift: Vec<PortableItemId> = Vec::new();
    for item in &merged.items {
        let Some(old) = cur_items.get(&item.id) else { continue };
        let guid = *ctx.items.get(&item.id).expect("checked above");
        if **old == *item {
            continue;
        }
        counts.items_changed += 1;
        let mut u = Update::new("item_instance", vec![("guid", Val::u(guid)), ("owner_guid", Val::Expr("@char"))]);
        if old.entry != item.entry {
            u.set("itemEntry", Val::u(content_id("item", &item.entry, ns, "item")?));
        }
        if old.count != item.count {
            u.set("count", Val::u(item.count));
        }
        if old.duration != item.duration {
            u.set("duration", Val::i(item.duration));
        }
        if old.charges != item.charges {
            u.set("charges", Val::opt_text(charges_text(item).as_deref()));
        }
        if old.flags != item.flags {
            u.set("flags", Val::u(item.flags));
        }
        if old.enchantments != item.enchantments {
            u.set("enchantments", Val::text(enchantment_text(item)));
        }
        if old.random_property_id != item.random_property_id {
            u.set("randomPropertyId", Val::i(item.random_property_id));
        }
        if old.durability != item.durability {
            u.set("durability", Val::u(item.durability));
        }
        if old.played_time != item.played_time {
            u.set("playedTime", Val::u(item.played_time));
        }
        if old.text != item.text {
            u.set("text", Val::opt_text(item.text.as_deref()));
        }
        st.update(u);
        if old.container != item.container || old.slot != item.slot {
            moved_inventory.push(item.id);
            counts.items_moved += 1;
        }
        if old.gift != item.gift {
            regift.push(item.id);
        }
    }
    counts.items_removed = removed_items.len();
    counts.items_added = added_items.len();

    // inventory rows of removed and moved items go first (the places they free may be taken by others), then come back
    let gone: Vec<Val> = removed_items.iter().chain(&moved_inventory).map(|id| ctx.items.get(id).map(|g| Val::u(*g)).expect("mapped")).collect();
    st.delete(Delete::any_of("character_inventory", char_key(), "item", gone));
    if !removed_items.is_empty() {
        let guids = |ids: &[PortableItemId]| -> Vec<Val> { ids.iter().map(|id| Val::u(*ctx.items.get(id).expect("mapped"))).collect() };
        st.delete(Delete::any_of("item_instance", vec![("owner_guid", Val::Expr("@char"))], "guid", guids(&removed_items)));
        st.delete(Delete::any_of("character_gifts", char_key(), "item_guid", guids(&removed_items)));
        if ctx.probe.has("item_refund_instance") {
            st.delete(Delete::any_of("item_refund_instance", vec![("player_guid", Val::Expr("@char"))], "item_guid", guids(&removed_items)));
        }
        if ctx.probe.has("item_soulbound_trade_data") {
            st.delete(Delete::any_of("item_soulbound_trade_data", vec![], "itemGuid", guids(&removed_items)));
        }
    }
    for id in &regift {
        st.delete(Delete::new("character_gifts", key_char(vec![("item_guid", Val::u(*ctx.items.get(id).expect("mapped")))])));
    }

    let mut item_rows = Insert::new(
        "item_instance",
        &["guid", "itemEntry", "owner_guid", "creatorGuid", "giftCreatorGuid", "count", "duration", "charges", "flags", "enchantments", "randomPropertyId", "durability", "playedTime", "text"],
    );
    let mut inventory = Insert::new("character_inventory", &["guid", "bag", "slot", "item"]);
    let mut gifts = Insert::new("character_gifts", &["guid", "item_guid", "entry", "flags"]);
    for id in &added_items {
        let item = new_items[id];
        item_rows.row(vec![
            local(id)?,
            Val::u(content_id("item", &item.entry, ns, "item")?),
            Val::Expr("@char"),
            Val::u(0u8), // the crafter named a character of another realm
            Val::u(0u8),
            Val::u(item.count),
            Val::i(item.duration),
            Val::opt_text(charges_text(item).as_deref()),
            Val::u(item.flags),
            Val::text(enchantment_text(item)),
            Val::i(item.random_property_id),
            Val::u(item.durability),
            Val::u(item.played_time),
            Val::opt_text(item.text.as_deref()),
        ])?;
        inventory.row(vec![Val::Expr("@char"), bag_of(item)?, Val::u(item.slot), local(id)?])?;
        if let Some(g) = &item.gift {
            gifts.row(vec![Val::Expr("@char"), local(id)?, Val::u(content_id("gift", &g.entry, ns, "item")?), Val::u(g.flags)])?;
        }
    }
    for id in &moved_inventory {
        let item = new_items[id];
        inventory.row(vec![Val::Expr("@char"), bag_of(item)?, Val::u(item.slot), local(id)?])?;
    }
    for id in &regift {
        if let Some(g) = &new_items[id].gift {
            gifts.row(vec![Val::Expr("@char"), local(id)?, Val::u(content_id("gift", &g.entry, ns, "item")?), Val::u(g.flags)])?;
        }
    }
    st.insert(item_rows);
    st.insert(inventory);
    st.insert(gifts);

    // ---- pets -------------------------------------------------------------------------------------------------------
    let cur_pets: HashMap<PortablePetId, &PortablePet> = current.pets.iter().map(|p| (p.id, p)).collect();
    let new_pets: HashMap<PortablePetId, &PortablePet> = merged.pets.iter().map(|p| (p.id, p)).collect();
    for id in cur_pets.keys() {
        if !ctx.pets.contains_key(id) {
            return Err(PortableError::Invalid(format!("internal: pet {id} of the realm has no local number")));
        }
    }
    let removed_pets: Vec<PortablePetId> = current.pets.iter().map(|p| p.id).filter(|id| !new_pets.contains_key(id)).collect();
    let added_pets: Vec<PortablePetId> = merged.pets.iter().map(|p| p.id).filter(|id| !cur_pets.contains_key(id)).collect();
    counts.pets_removed = removed_pets.len();
    counts.pets_added = added_pets.len();

    if !removed_pets.is_empty() {
        let numbers: Vec<Val> = removed_pets.iter().map(|id| Val::u(*ctx.pets.get(id).expect("mapped"))).collect();
        st.delete(Delete::any_of("character_pet", vec![("owner", Val::Expr("@char"))], "id", numbers.clone()));
        st.delete(Delete::any_of("pet_spell", vec![], "guid", numbers.clone()));
        for table in ["pet_aura", "pet_spell_cooldown"] {
            if ctx.probe.has(table) {
                st.delete(Delete::any_of(table, vec![], "guid", numbers.clone()));
            }
        }
        if ctx.probe.has("character_pet_declinedname") {
            st.delete(Delete::any_of("character_pet_declinedname", vec![("owner", Val::Expr("@char"))], "id", numbers));
        }
    }
    let mut pet_spell_rows = Insert::new("pet_spell", &["guid", "spell", "active"]);
    let mut declined = Insert::new("character_pet_declinedname", &["id", "owner", "genitive", "dative", "accusative", "instrumental", "prepositional"]);
    for pet in &merged.pets {
        let Some(old) = cur_pets.get(&pet.id) else { continue };
        let number = *ctx.pets.get(&pet.id).expect("checked above");
        if **old == *pet {
            continue;
        }
        if old.entry != pet.entry || old.pet_type != pet.pet_type || old.created_by_spell != pet.created_by_spell {
            return Err(cannot("what a pet is (creature, kind or summoning spell)"));
        }
        counts.pets_changed += 1;
        let mut u = Update::new("character_pet", vec![("id", Val::u(number)), ("owner", Val::Expr("@char"))]);
        if old.model_id != pet.model_id {
            u.set("modelid", Val::u(pet.model_id));
        }
        if old.level != pet.level {
            u.set("level", Val::u(pet.level));
        }
        if old.exp != pet.exp {
            u.set("exp", Val::u(pet.exp));
        }
        if old.react_state != pet.react_state {
            u.set("Reactstate", Val::u(pet.react_state));
        }
        if old.name != pet.name {
            u.set("name", Val::text(pet.name.clone()));
        }
        if old.renamed != pet.renamed {
            u.set("renamed", Val::u(pet.renamed as u8));
        }
        if old.slot != pet.slot {
            u.set("slot", Val::u(pet.slot));
        }
        if old.health != pet.health {
            u.set("curhealth", Val::u(pet.health));
        }
        if old.mana != pet.mana {
            u.set("curmana", Val::u(pet.mana));
        }
        if old.happiness != pet.happiness {
            u.set("curhappiness", Val::u(pet.happiness));
        }
        if old.action_bar != pet.action_bar {
            u.set("abdata", if pet.action_bar.is_empty() { Val::Null } else { Val::text(pet.action_bar.clone()) });
        }
        st.update(u);
        let spells = |p: &PortablePet| -> BTreeMap<u32, u8> { p.spells.iter().map(|s| (s.spell, s.active)).collect() };
        let (del, ins) = diff(&spells(old), &spells(pet));
        counts.keyed_rows += del.len() + ins.len();
        st.delete(Delete::any_of("pet_spell", vec![("guid", Val::u(number))], "spell", del.iter().map(|s| Val::u(*s)).collect()));
        for (spell, active) in ins {
            pet_spell_rows.row(vec![Val::u(number), Val::u(spell), Val::u(active)])?;
        }
        if old.declined_names != pet.declined_names {
            counts.keyed_rows += 1;
            st.delete(Delete::new("character_pet_declinedname", vec![("id", Val::u(number)), ("owner", Val::Expr("@char"))]));
            if let Some(names) = &pet.declined_names {
                let mut r = vec![Val::u(number), Val::Expr("@char")];
                r.extend(names.iter().map(|n| Val::text(n.clone())));
                declined.row(r)?;
            }
        }
    }
    let mut pet_rows = Insert::new(
        "character_pet",
        &["id", "entry", "owner", "modelid", "CreatedBySpell", "PetType", "level", "exp", "Reactstate", "name", "renamed", "slot", "curhealth", "curmana", "curhappiness", "savetime", "abdata"],
    );
    for (i, id) in added_pets.iter().enumerate() {
        let pet = new_pets[id];
        let i = i as u32;
        pet_rows.row(vec![
            Val::PetRef(i),
            Val::u(content_id("pet", &pet.entry, ns, "creature")?),
            Val::Expr("@char"),
            Val::u(pet.model_id),
            Val::u(pet.created_by_spell),
            Val::u(pet.pet_type),
            Val::u(pet.level),
            Val::u(pet.exp),
            Val::u(pet.react_state),
            Val::text(pet.name.clone()),
            Val::u(pet.renamed as u8),
            Val::u(pet.slot),
            Val::u(pet.health),
            Val::u(pet.mana),
            Val::u(pet.happiness),
            Val::Expr("UNIX_TIMESTAMP()"),
            if pet.action_bar.is_empty() { Val::Null } else { Val::text(pet.action_bar.clone()) },
        ])?;
        for s in &pet.spells {
            pet_spell_rows.row(vec![Val::PetRef(i), Val::u(s.spell), Val::u(s.active)])?;
        }
        if let Some(names) = &pet.declined_names {
            let mut r = vec![Val::PetRef(i), Val::Expr("@char")];
            r.extend(names.iter().map(|n| Val::text(n.clone())));
            declined.row(r)?;
        }
    }
    st.insert(pet_rows);
    st.insert(pet_spell_rows);
    st.insert(declined);

    // ---- the script -------------------------------------------------------------------------------------------------
    let marker = super::plan::marker_data(ctx.nonce, ctx.revision);
    let mut s = String::new();
    let mut line = |text: String| {
        s.push_str(&text);
        s.push('\n');
    };
    line(format!("DO IF(GET_LOCK('{IMPORT_LOCK}', 10) = 1, 1, {FAIL});"));
    line("SET SESSION innodb_lock_wait_timeout = 10;".into());
    line("START TRANSACTION;".into());
    line(format!("DO IF((SELECT COUNT(*) FROM acore_characters.characters WHERE online <> 0) = 0, 1, {FAIL});"));
    let users = ctx.game_server_users.iter().map(|u| Val::text(u.clone()).sql()).collect::<Vec<_>>().join(", ");
    line(format!("DO IF((SELECT COUNT(*) FROM information_schema.processlist WHERE id <> CONNECTION_ID() AND user IN ({users})) = 0, 1, {FAIL});"));
    line(format!("SET @char := {};", ctx.local_guid));
    // the character is what was read: another writer between the read and this transaction fails the update instead of being overwritten
    line(format!(
        "DO IF((SELECT COUNT(*) FROM acore_characters.characters WHERE guid = @char AND deleteDate IS NULL AND level = {} AND xp = {} AND money = {} AND name = {}) = 1, 1, {FAIL});",
        p.level,
        p.xp,
        p.money,
        Val::text(ci.name.clone()).sql()
    ));
    line("SET @items_before := (SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = @char);".into());
    line("SET @inventory_before := (SELECT COUNT(*) FROM acore_characters.character_inventory WHERE guid = @char);".into());
    line("SET @pets_before := (SELECT COUNT(*) FROM acore_characters.character_pet WHERE owner = @char);".into());
    // rows the realm itself would clean up on load (an inventory row without its item, an item in a bag that is not there) may
    // exist before the update; the update must neither create nor repair them, so the assertions compare against these
    line(format!("SET @orphans_before := {ORPHAN_ITEMS};"));
    line(format!("SET @bagless_before := {BAGLESS_ITEMS};"));
    line(format!("SET @item_base := {} + 1;", max_of(ctx.probe, ITEM_KEYED)));
    line(format!("SET @pet_base := {} + 1;", max_of(ctx.probe, PET_KEYED)));
    line(format!("DO IF(@item_base + {} < 4294967000 AND @pet_base + {} < 4294967000, 1, {FAIL});", added_items.len(), added_pets.len()));
    for g in &st.guards {
        line(g.clone());
    }
    for sql in st.deletes.iter().chain(&st.updates).chain(&st.inserts) {
        line(sql.clone());
    }

    // the transaction checks its own result
    line(format!("DO IF((SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = @char) = @items_before + {} - {}, 1, {FAIL});", added_items.len(), removed_items.len()));
    line(format!("DO IF((SELECT COUNT(*) FROM acore_characters.character_inventory WHERE guid = @char) = @inventory_before + {} - {}, 1, {FAIL});", added_items.len(), removed_items.len()));
    line(format!("DO IF((SELECT COUNT(*) FROM acore_characters.character_pet WHERE owner = @char) = @pets_before + {} - {}, 1, {FAIL});", added_pets.len(), removed_pets.len()));
    line(format!("DO IF({ORPHAN_ITEMS} = @orphans_before, 1, {FAIL});"));
    line(format!("DO IF({BAGLESS_ITEMS} = @bagless_before, 1, {FAIL});"));
    line(format!("DO IF((SELECT COUNT(*) FROM acore_characters.characters WHERE guid = @char) = 1, 1, {FAIL});"));

    // the markers: replaced, not added
    let markers = [Val::text(IMPORT_MARKER_SOURCE), Val::text(ALLOC_MARKER_SOURCE)];
    line(format!("DELETE FROM acore_characters.`character_settings` WHERE `guid` = @char AND `source` IN ({}, {});", markers[0].sql(), markers[1].sql()));
    let mut m = Insert::new("character_settings", &["guid", "source", "data"]);
    m.row(vec![Val::Expr("@char"), markers[0].clone(), Val::text(marker.clone())])?;
    m.row(vec![Val::Expr("@char"), markers[1].clone(), Val::Expr("CONCAT(@item_base, ' ', @pet_base, ' ')")])?;
    line(m.sql().expect("two rows"));

    line("SELECT '#R:update', @char, @item_base, @pet_base;".into());
    line("COMMIT;".into());
    line("SELECT '#R:committed';".into());
    line(format!("DO RELEASE_LOCK('{IMPORT_LOCK}');"));

    Ok(UpdateScript { script: s, added_items, added_pets, removed_items, removed_pets, counts, marker_data: marker })
}

/// What the realm answered after a successful update: (local guid, item base, pet base, committed).
pub fn parse_update_report(output: &str) -> Result<((u32, u32, u32), bool)> {
    let mut found = None;
    let mut committed = false;
    for line in output.lines().filter(|l| !l.is_empty()) {
        let cells: Vec<&str> = line.split('\t').collect();
        match cells[0] {
            "#R:update" if cells.len() == 4 => {
                let num = |i: usize| cells[i].parse::<u32>().map_err(|_| PortableError::CorruptSnapshot(format!("update report: {:?} is not a number", cells[i])));
                found = Some((num(1)?, num(2)?, num(3)?));
            }
            "#R:committed" => committed = true,
            other => return Err(PortableError::CorruptSnapshot(format!("unexpected line in the update report: {other:?}"))),
        }
    }
    Ok((found.ok_or_else(|| PortableError::CorruptSnapshot("the update report has no allocation".into()))?, committed))
}

/// Items the update will give the realm that its `item_template` must know, and pet creatures: for the preflight.
pub fn new_content(merged: &PortableCharacter, added_items: &[PortableItemId], added_pets: &[PortablePetId]) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let items: BTreeSet<u32> = merged
        .items
        .iter()
        .filter(|i| added_items.contains(&i.id))
        .flat_map(|i| std::iter::once(i.entry.id()).chain(i.gift.as_ref().map(|g| g.entry.id())))
        .filter_map(|e| u32::try_from(e).ok())
        .collect();
    let pets: BTreeSet<u32> = merged.pets.iter().filter(|p| added_pets.contains(&p.id)).filter_map(|p| u32::try_from(p.entry.id()).ok()).collect();
    (items, pets)
}
