//! A portable character -> the fixed SQL script that imports it into a **stopped** realm, in **one MySQL transaction**.
//!
//! The script is generated, never hand-assembled from strings:
//! * every value goes through the typed encoder ([`super::sqlenc`]): untrusted text and blobs exist in the script only as
//!   hex literals;
//! * every id the realm owns is allocated *inside* the transaction from the realm's own tables (`@char`, `@item_base`,
//!   `@pet_base`) and every reference is remapped through them (`@item_base + index`), so no guid of the source realm
//!   (or of the canonical snapshot) ever reaches the destination;
//! * the transaction asserts its own preconditions and results (an assertion that fails raises an SQL error, which aborts
//!   the script before `COMMIT`, which rolls everything back).
//!
//! Layout of the script: lock -> guards -> allocation -> start position -> name -> orphan cleanup -> inserts ->
//! assertions -> marker -> `COMMIT` -> report. Nothing is written outside the transaction.

use std::collections::HashMap;

use super::super::error::{PortableError, Result};
use super::super::ids::{ContentId, PortableItemId, PortablePetId};
use super::super::model::*;
use super::policy::{classify_setting, Disposition, IMPORT_MARKER_SOURCE};
use super::script::SchemaProbe;
use super::sqlenc::{Insert, Val};

pub const IMPORT_LOCK: &str = "coa_portable_import";
/// Longest character name the characters table holds.
pub const NAME_COLUMN_CHARS: u32 = 25;
/// Health/power written for a fresh arrival: the realm clamps to the maximum on load, so this means "full".
const FULL: u64 = 2_000_000_000;

/// Raises an SQL error ("subquery returns more than 1 row"): used as the failing branch of an assertion.
pub(super) const FAIL: &str = "(SELECT 1 UNION ALL SELECT 2)";

/// Per-character tables whose rows are keyed by the **character guid**. Rows left there by a deleted character with
/// the same guid are removed before the import (the realm's own delete leaves module tables behind). Curated by hand:
/// a column named `guid` is not always a character guid (`groups.guid`, `creature_respawn.guid`, ...).
pub const CHARACTER_KEYED: &[(&str, &str)] = &[
    ("ascension_manastorm_bonus", "guid"),
    ("ascension_manastorm_cache", "guid"),
    ("ascension_manastorm_clear", "guid"),
    ("ascension_manastorm_loadout", "guid"),
    ("ascension_manastorm_xp", "guid"),
    ("character_account_data", "guid"),
    ("character_achievement", "guid"),
    ("character_achievement_offline_updates", "guid"),
    ("character_achievement_progress", "guid"),
    ("character_action", "guid"),
    ("character_appearance", "guid"),
    ("character_appearance_outfit", "guid"),
    ("character_appearance_settings", "guid"),
    ("character_arena_stats", "guid"),
    ("character_ascension_state", "guid"),
    ("character_aura", "guid"),
    ("character_banned", "guid"),
    ("character_battleground_random", "guid"),
    ("character_brew_of_the_month", "guid"),
    ("character_coa_lfg_settings", "guid"),
    ("character_declinedname", "guid"),
    ("character_entry_point", "guid"),
    ("character_equipmentsets", "guid"),
    ("character_gifts", "guid"),
    ("character_glyphs", "guid"),
    ("character_homebind", "guid"),
    ("character_instance", "guid"),
    ("character_inventory", "guid"),
    ("character_pet", "owner"),
    ("character_pet_declinedname", "owner"),
    ("character_queststatus", "guid"),
    ("character_queststatus_daily", "guid"),
    ("character_queststatus_monthly", "guid"),
    ("character_queststatus_rewarded", "guid"),
    ("character_queststatus_seasonal", "guid"),
    ("character_queststatus_weekly", "guid"),
    ("character_reputation", "guid"),
    ("character_settings", "guid"),
    ("character_skills", "guid"),
    ("character_spell", "guid"),
    ("character_spell_cooldown", "guid"),
    ("character_stats", "guid"),
    ("character_talent", "guid"),
    ("character_worldforged_loot", "guid"),
    ("coa_challenge_completion", "guid"),
    ("coa_challenge_failure", "guid"),
    ("coa_character_challenge", "guid"),
    ("coa_character_condition", "guid"),
    ("coa_character_fatigue", "guid"),
    ("coa_character_gamemode", "guid"),
    ("coa_character_gamemode_lives", "guid"),
    ("coa_character_looted_item", "guid"),
    ("coa_character_objective", "guid"),
    ("coa_character_survival", "guid"),
    ("coa_custom_trial", "guid"),
    ("coa_custom_trial_active", "guid"),
    ("coa_custom_trial_completion", "guid"),
    ("coa_custom_trial_entry", "guid"),
    ("coa_custom_trial_vote", "guid"),
    ("corpse", "guid"),
    ("item_instance", "owner_guid"),
    ("mod_craftsmans_codex", "guid"),
];

/// Tables that name **item guids**. The new item guids start above the highest of all of them, so a stale row (a mail,
/// an auction, a bank slot of a long-deleted item) can never attach itself to an imported item.
pub const ITEM_KEYED: &[(&str, &str)] = &[
    ("item_instance", "guid"),
    ("character_inventory", "item"),
    ("character_gifts", "item_guid"),
    ("mail_items", "item_guid"),
    ("auctionhouse", "itemguid"),
    ("guild_bank_item", "item_guid"),
    ("item_refund_instance", "item_guid"),
    ("item_soulbound_trade_data", "itemGuid"),
    ("item_loot_storage", "containerGUID"),
    ("ascension_manastorm_cache", "item"),
    ("highrisk_chest_item", "item_guid"),
    ("mod_ascension_bank_item", "item_guid"),
    ("coa_character_looted_item", "itemGuid"),
];

/// Tables that name **pet numbers**.
pub const PET_KEYED: &[(&str, &str)] = &[
    ("character_pet", "id"),
    ("pet_spell", "guid"),
    ("pet_aura", "guid"),
    ("pet_spell_cooldown", "guid"),
    ("character_pet_declinedname", "id"),
];

pub struct PlanContext<'a> {
    pub ruleset: Ruleset,
    /// The destination game account (it must exist; checked in the script and by the preflight).
    pub account: u32,
    /// The canonical revision being imported.
    pub revision: u64,
    /// Identifies this import in the realm (see [`marker_data`]).
    pub nonce: [u32; 4],
    pub max_characters_per_account: u32,
    /// Database users of a running game server; any other session of one of these users means the realm is running.
    pub game_server_users: &'a [String],
    pub probe: &'a SchemaProbe,
}

/// What the plan will write, for assertions and reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanCounts {
    pub items: usize,
    pub inventory: usize,
    pub gifts: usize,
    pub spells: usize,
    pub talents: usize,
    pub skills: usize,
    pub glyphs: usize,
    pub reputation: usize,
    pub quests: usize,
    pub rewarded: usize,
    pub actions: usize,
    pub pets: usize,
    pub pet_spells: usize,
    pub declined: usize,
    pub settings: usize,
    pub macros: usize,
}

#[derive(Debug)]
pub struct ImportPlan {
    pub script: String,
    /// Item `i` of the plan gets the realm guid `@item_base + i`.
    pub item_ids: Vec<PortableItemId>,
    /// Pet `i` of the plan gets the pet number `@pet_base + i`.
    pub pet_ids: Vec<PortablePetId>,
    pub marker_data: String,
    pub counts: PlanCounts,
    /// Settings sources that are in the snapshot but are deliberately not applied.
    pub not_applied_settings: Vec<String>,
}

/// The text stored in the realm's `character_settings` row `coa.portable.import` of an imported character: four random
/// words and the canonical revision, all numbers. Recovery finds the character by this exact text.
pub fn marker_data(nonce: [u32; 4], revision: u64) -> String {
    format!("{} {} {} {} {} ", nonce[0], nonce[1], nonce[2], nonce[3], revision)
}

pub(super) fn to_u32(what: &str, v: u64) -> Result<u32> {
    u32::try_from(v).map_err(|_| PortableError::Invalid(format!("{what} {v} does not fit the realm's 32-bit column")))
}

pub(super) fn content_id(what: &str, id: &ContentId, ns: &str, kind: &str) -> Result<u32> {
    if id.namespace() != ns || id.kind() != kind {
        return Err(PortableError::Invalid(format!("{what} {id} is not a {ns}:{kind} id and cannot be imported into a {ns} realm")));
    }
    to_u32(what, id.id())
}

pub(super) fn settings_text(values: &[u32]) -> String {
    values.iter().map(|v| format!("{v} ")).collect()
}

/// `a b c ... ` for the 12 enchantment slots, as the realm stores them.
pub(super) fn enchantment_text(item: &PortableItem) -> String {
    let mut slots = [[0u32; 3]; 12];
    for e in &item.enchantments {
        slots[e.slot as usize] = [e.id, e.duration, e.charges];
    }
    slots.iter().flatten().map(|v| format!("{v} ")).collect()
}

pub(super) fn charges_text(item: &PortableItem) -> Option<String> {
    if item.charges.is_empty() {
        return None;
    }
    let mut c = item.charges.clone();
    c.resize(5, 0);
    Some(c.iter().map(|v| format!("{v} ")).collect())
}

fn assert_count(table_sql: &str, expected: usize) -> String {
    format!("DO IF((SELECT COUNT(*) FROM {table_sql}) = {expected}, 1, {FAIL});")
}

pub fn build_plan(model: &PortableCharacter, ctx: &PlanContext<'_>) -> Result<ImportPlan> {
    model.validate()?;
    if model.ruleset != ctx.ruleset {
        return Err(PortableError::WrongRealm { expected: model.ruleset.to_string(), found: ctx.ruleset.to_string() });
    }
    let ns = ctx.ruleset.as_str();
    if model.content_namespace != ns {
        return Err(PortableError::Invalid(format!("the character's content namespace {} is not {ns}", model.content_namespace)));
    }
    if ctx.game_server_users.is_empty() {
        return Err(PortableError::Invalid("internal: no game server user is named, the realm cannot be proven stopped".into()));
    }

    let race = content_id("race", &model.identity.race, ns, "race")?;
    let class = content_id("class", &model.identity.class, ns, "class")?;
    let item_index: HashMap<PortableItemId, u32> = model.items.iter().enumerate().map(|(i, it)| (it.id, i as u32)).collect();

    // ---- rows -----------------------------------------------------------------------------------------------------
    let mut counts = PlanCounts::default();
    let mut inserts: Vec<Insert> = Vec::new();

    let p = &model.progression;
    let mut characters = Insert::new(
        "characters",
        &[
            "guid", "account", "name", "race", "class", "gender", "level", "xp", "money", "skin", "face", "hairStyle", "hairColor", "facialStyle", "bankSlots", "restState",
            "playerFlags", "position_x", "position_y", "position_z", "map", "instance_id", "instance_mode_mask", "orientation", "taximask", "online", "cinematic", "totaltime",
            "leveltime", "logout_time", "is_logout_resting", "rest_bonus", "resettalents_cost", "resettalents_time", "extra_flags", "stable_slots", "at_login", "zone",
            "death_expire_time", "arenaPoints", "totalHonorPoints", "todayHonorPoints", "yesterdayHonorPoints", "totalKills", "todayKills", "yesterdayKills", "chosenTitle",
            "knownCurrencies", "watchedFaction", "drunk", "health", "power1", "power2", "power3", "power4", "power5", "power6", "power7", "latency", "talentGroupsCount",
            "activeTalentGroup", "exploredZones", "equipmentCache", "ammoId", "knownTitles", "actionBars", "grantableLevels", "innTriggerId", "extraBonusTalentCount",
        ],
    );
    let a = &model.identity.appearance;
    characters.row(vec![
        Val::Expr("@char"),
        Val::u(ctx.account),
        Val::Expr("@final_name"),
        Val::u(race),
        Val::u(class),
        Val::u(model.identity.gender),
        Val::u(p.level),
        Val::u(p.xp),
        Val::u(p.money),
        Val::u(a.skin),
        Val::u(a.face),
        Val::u(a.hair_style),
        Val::u(a.hair_color),
        Val::u(a.facial_style),
        Val::u(p.bank_slots),
        Val::u(0u8),
        Val::u(model.identity.cosmetic_flags),
        Val::Expr("@x"),
        Val::Expr("@y"),
        Val::Expr("@z"),
        Val::Expr("@map"),
        Val::u(0u8),
        Val::u(0u8),
        Val::Expr("@o"),
        Val::text(p.taxi_mask.clone()),
        Val::u(0u8),
        Val::u(1u8), // the intro cinematic has been seen
        Val::u(0u8),
        Val::u(0u8),
        Val::u(0u8),
        Val::u(0u8),
        Val::u(0u8),
        Val::u(model.build.reset_talents_cost),
        Val::u(0u8),
        Val::u(0u8),
        Val::u(p.stable_slots),
        Val::Expr("@at_login"),
        Val::Expr("@zone"),
        Val::u(0u8),
        Val::u(p.honor.arena_points),
        Val::u(p.honor.total_honor),
        Val::u(p.honor.today_honor),
        Val::u(p.honor.yesterday_honor),
        Val::u(p.honor.total_kills),
        Val::u(p.honor.today_kills),
        Val::u(p.honor.yesterday_kills),
        Val::u(p.chosen_title),
        Val::u(p.known_currencies),
        Val::u(p.watched_faction),
        Val::u(0u8),
        Val::u(FULL), // health
        Val::u(FULL), // power1 (mana)
        Val::u(0u8),  // power2 (rage)
        Val::u(0u8),  // power3 (focus)
        Val::u(FULL), // power4 (energy)
        Val::u(0u8),
        Val::u(0u8),
        Val::u(0u8),
        Val::u(0u8),
        Val::u(model.build.talent_groups_count),
        Val::u(model.build.active_talent_group),
        Val::text(p.explored_zones.clone()),
        Val::text(""),
        Val::u(0u8),
        Val::text(p.known_titles.clone()),
        Val::u(p.action_bars_mask),
        Val::u(0u8),
        Val::u(0u8),
        Val::i(model.build.extra_bonus_talent_count),
    ])?;
    inserts.push(characters);

    let mut homebind = Insert::new("character_homebind", &["guid", "mapId", "zoneId", "posX", "posY", "posZ"]);
    homebind.row(vec![Val::Expr("@char"), Val::Expr("@map"), Val::Expr("@zone"), Val::Expr("@x"), Val::Expr("@y"), Val::Expr("@z")])?;
    inserts.push(homebind);

    let mut item_rows = Insert::new(
        "item_instance",
        &["guid", "itemEntry", "owner_guid", "creatorGuid", "giftCreatorGuid", "count", "duration", "charges", "flags", "enchantments", "randomPropertyId", "durability", "playedTime", "text"],
    );
    let mut inventory = Insert::new("character_inventory", &["guid", "bag", "slot", "item"]);
    let mut gifts = Insert::new("character_gifts", &["guid", "item_guid", "entry", "flags"]);
    for (i, item) in model.items.iter().enumerate() {
        let entry = content_id("item", &item.entry, ns, "item")?;
        item_rows.row(vec![
            Val::ItemRef(i as u32),
            Val::u(entry),
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
        let bag = match item.container {
            None => Val::u(0u8),
            Some(c) => Val::ItemRef(*item_index.get(&c).ok_or_else(|| PortableError::Invalid("an item names a container that is not part of the plan".into()))?),
        };
        inventory.row(vec![Val::Expr("@char"), bag, Val::u(item.slot), Val::ItemRef(i as u32)])?;
        if let Some(g) = &item.gift {
            gifts.row(vec![Val::Expr("@char"), Val::ItemRef(i as u32), Val::u(content_id("gift", &g.entry, ns, "item")?), Val::u(g.flags)])?;
        }
    }
    counts.items = item_rows.len();
    counts.inventory = inventory.len();
    counts.gifts = gifts.len();
    inserts.extend([item_rows, inventory, gifts]);

    let mut spells = Insert::new("character_spell", &["guid", "spell", "specMask"]);
    for (spell, mask) in &model.build.spells {
        spells.row(vec![Val::Expr("@char"), Val::u(*spell), Val::u(*mask)])?;
    }
    // Stock talents are NOT written. The realm asserts (and the whole worldserver dies) when `character_talent` names a spell
    // that is not a talent of its DBC, and nothing in the realm's database can tell whether a spell id is one. CoA keeps its
    // talents as spells and its builds in `character_settings`; real CoA characters have no `character_talent` rows at all.
    // The rows stay in the canonical snapshot.
    let mut not_applied_talents = Vec::new();
    if !model.build.talents.is_empty() {
        not_applied_talents.push(format!("character_talent ({} stock talent rows: not applied, see PORTABLE_IMPORT.md)", model.build.talents.len()));
    }
    let mut skills = Insert::new("character_skills", &["guid", "skill", "value", "max"]);
    for s in &model.build.skills {
        skills.row(vec![Val::Expr("@char"), Val::u(s.skill), Val::u(s.value), Val::u(s.max)])?;
    }
    let mut glyphs = Insert::new("character_glyphs", &["guid", "talentGroup", "glyph1", "glyph2", "glyph3", "glyph4", "glyph5", "glyph6"]);
    for g in &model.build.glyphs {
        let mut row = vec![Val::Expr("@char"), Val::u(g.talent_group)];
        row.extend(g.glyphs.iter().map(|v| Val::u(*v)));
        glyphs.row(row)?;
    }
    (counts.spells, counts.talents, counts.skills, counts.glyphs) = (spells.len(), 0, skills.len(), glyphs.len());
    inserts.extend([spells, skills, glyphs]);

    let mut reputation = Insert::new("character_reputation", &["guid", "faction", "standing", "flags"]);
    for r in &model.reputation {
        reputation.row(vec![Val::Expr("@char"), Val::u(r.faction), Val::i(r.standing), Val::u(r.flags)])?;
    }
    let mut quests = Insert::new(
        "character_queststatus",
        &["guid", "quest", "status", "explored", "timer", "mobcount1", "mobcount2", "mobcount3", "mobcount4", "itemcount1", "itemcount2", "itemcount3", "itemcount4", "itemcount5", "itemcount6", "playercount"],
    );
    for q in &model.quests.active {
        let mut row = vec![Val::Expr("@char"), Val::u(q.quest), Val::u(q.status), Val::u(q.explored as u8), Val::u(q.timer)];
        row.extend(q.mob_counts.iter().map(|v| Val::u(*v)));
        row.extend(q.item_counts.iter().map(|v| Val::u(*v)));
        row.push(Val::u(q.player_count));
        quests.row(row)?;
    }
    let mut rewarded = Insert::new("character_queststatus_rewarded", &["guid", "quest", "active"]);
    for q in &model.quests.rewarded {
        rewarded.row(vec![Val::Expr("@char"), Val::u(*q), Val::u(1u8)])?;
    }
    let mut actions = Insert::new("character_action", &["guid", "spec", "button", "action", "type"]);
    for a in &model.actions {
        actions.row(vec![Val::Expr("@char"), Val::u(a.spec), Val::u(a.button), Val::u(a.action), Val::u(a.kind)])?;
    }
    counts.reputation = reputation.len();
    counts.quests = quests.len();
    counts.rewarded = rewarded.len();
    counts.actions = actions.len();
    inserts.extend([reputation, quests, rewarded, actions]);

    let mut pets = Insert::new(
        "character_pet",
        &["id", "entry", "owner", "modelid", "CreatedBySpell", "PetType", "level", "exp", "Reactstate", "name", "renamed", "slot", "curhealth", "curmana", "curhappiness", "savetime", "abdata"],
    );
    let mut pet_spells = Insert::new("pet_spell", &["guid", "spell", "active"]);
    let mut declined = Insert::new("character_pet_declinedname", &["id", "owner", "genitive", "dative", "accusative", "instrumental", "prepositional"]);
    for (i, pet) in model.pets.iter().enumerate() {
        let i = i as u32;
        pets.row(vec![
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
            pet_spells.row(vec![Val::PetRef(i), Val::u(s.spell), Val::u(s.active)])?;
        }
        if let Some(names) = &pet.declined_names {
            let mut row = vec![Val::PetRef(i), Val::Expr("@char")];
            row.extend(names.iter().map(|n| Val::text(n.clone())));
            declined.row(row)?;
        }
    }
    (counts.pets, counts.pet_spells, counts.declined) = (pets.len(), pet_spells.len(), declined.len());
    inserts.extend([pets, pet_spells, declined]);

    // settings: only what the policy carries is applied; the importer's own marker goes in the same table
    let mut settings = Insert::new("character_settings", &["guid", "source", "data"]);
    let mut not_applied = not_applied_talents;
    for (source, values) in &model.settings {
        match classify_setting(source, model.ruleset) {
            Disposition::Carry => settings.row(vec![Val::Expr("@char"), Val::text(source.clone()), Val::text(settings_text(values))])?,
            _ => not_applied.push(source.clone()),
        }
    }
    // extensions (module data this importer does not apply) stay in the snapshot only
    not_applied.extend(model.extensions.keys().cloned());
    counts.settings = settings.len();
    let marker = marker_data(ctx.nonce, ctx.revision);
    settings.row(vec![Val::Expr("@char"), Val::text(IMPORT_MARKER_SOURCE), Val::text(marker.clone())])?;
    inserts.push(settings);

    let mut macros = Insert::new("character_account_data", &["guid", "type", "time", "data"]);
    if let Some(blob) = model.client_data.get(&5) {
        macros.row(vec![Val::Expr("@char"), Val::u(5u8), Val::u(blob.time), Val::Bytes(blob.data.0.clone())])?;
    }
    counts.macros = macros.len();
    inserts.push(macros);

    // ---- the script -----------------------------------------------------------------------------------------------
    let mut s = String::new();
    let mut line = |text: String| {
        s.push_str(&text);
        s.push('\n');
    };
    let has = |t: &str| ctx.probe.has(t);

    line(format!("DO IF(GET_LOCK('{IMPORT_LOCK}', 10) = 1, 1, {FAIL});"));
    line("SET SESSION innodb_lock_wait_timeout = 10;".into());
    line("START TRANSACTION;".into());

    // guards: the realm is stopped (no online character, no session of the game server's database user), the account exists
    line(format!("DO IF((SELECT COUNT(*) FROM acore_characters.characters WHERE online <> 0) = 0, 1, {FAIL});"));
    let users = ctx.game_server_users.iter().map(|u| Val::text(u.clone()).sql()).collect::<Vec<_>>().join(", ");
    line(format!("DO IF((SELECT COUNT(*) FROM information_schema.processlist WHERE id <> CONNECTION_ID() AND user IN ({users})) = 0, 1, {FAIL});"));
    line(format!("DO IF((SELECT COUNT(*) FROM acore_auth.account WHERE id = {}) = 1, 1, {FAIL});", ctx.account));
    line(format!("DO IF((SELECT COUNT(*) FROM acore_auth.account WHERE id = {} AND UPPER(username) NOT LIKE 'COABOT%' AND UPPER(username) <> 'COAMANAGER') = 1, 1, {FAIL});", ctx.account));
    line(format!("DO IF((SELECT COUNT(*) FROM acore_characters.characters WHERE account = {}) < {}, 1, {FAIL});", ctx.account, ctx.max_characters_per_account));

    // allocation: above everything that exists (and everything stale rows still name)
    line("SET @char := (SELECT IFNULL(MAX(guid), 0) + 1 FROM acore_characters.characters);".into());
    let max_of = |list: &[(&str, &str)]| -> String {
        let parts: Vec<String> = list.iter().filter(|(t, _)| has(t)).map(|(t, c)| format!("IFNULL((SELECT MAX(`{c}`) FROM acore_characters.`{t}`), 0)")).collect();
        format!("GREATEST(0, {})", parts.join(", "))
    };
    line(format!("SET @item_base := {} + 1;", max_of(ITEM_KEYED)));
    line(format!("SET @pet_base := {} + 1;", max_of(PET_KEYED)));
    line(format!("DO IF(@char < 4294967000 AND @item_base + {} < 4294967000 AND @pet_base + {} < 4294967000, 1, {FAIL});", model.items.len(), model.pets.len()));

    // start position from the destination's own world data
    line(format!("SELECT map, zone, position_x, position_y, position_z, orientation INTO @map, @zone, @x, @y, @z, @o FROM acore_world.playercreateinfo WHERE race = {race} AND class = {class};"));
    line(format!("DO IF(@map IS NOT NULL, 1, {FAIL});"));

    // name: kept unless it collides (case/accent-insensitively) or is reserved; then a temporary name + rename at login
    line(format!("SET @name := {};", Val::text(model.identity.name.clone()).sql()));
    let collides = "EXISTS(SELECT 1 FROM acore_characters.characters WHERE name COLLATE utf8mb4_unicode_ci = @name COLLATE utf8mb4_unicode_ci)";
    let reserved = if has("reserved_name") { " OR EXISTS(SELECT 1 FROM acore_characters.reserved_name WHERE name COLLATE utf8mb4_unicode_ci = @name COLLATE utf8mb4_unicode_ci)" } else { "" };
    line(format!("SET @rename := IF({collides}{reserved}, 1, 0);"));
    line(format!("SET @final_name := IF(@rename = 1, CONCAT(LEFT(@name, {NAME_COLUMN_CHARS} - CHAR_LENGTH(HEX(@char))), HEX(@char)), @name);"));
    line("SET @at_login := IF(@rename = 1, 1, 0);".into());

    // leftovers of a deleted character that had this guid
    for (table, column) in CHARACTER_KEYED.iter().filter(|(t, _)| has(t)) {
        line(format!("DELETE FROM acore_characters.`{table}` WHERE `{column}` = @char;"));
    }

    for insert in &inserts {
        if let Some(sql) = insert.sql() {
            line(sql);
        }
    }

    // the transaction checks its own result: a failed assertion raises an SQL error, so there is no COMMIT
    line(assert_count("acore_characters.characters WHERE guid = @char", 1));
    line(assert_count("acore_characters.item_instance WHERE owner_guid = @char", counts.items));
    line(assert_count("acore_characters.character_inventory WHERE guid = @char", counts.inventory));
    line(assert_count("acore_characters.character_spell WHERE guid = @char", counts.spells));
    line(assert_count("acore_characters.character_talent WHERE guid = @char", counts.talents));
    line(assert_count("acore_characters.character_skills WHERE guid = @char", counts.skills));
    line(assert_count("acore_characters.character_reputation WHERE guid = @char", counts.reputation));
    line(assert_count("acore_characters.character_queststatus WHERE guid = @char", counts.quests));
    line(assert_count("acore_characters.character_queststatus_rewarded WHERE guid = @char", counts.rewarded));
    line(assert_count("acore_characters.character_action WHERE guid = @char", counts.actions));
    line(assert_count("acore_characters.character_pet WHERE owner = @char", counts.pets));
    line(assert_count("acore_characters.character_settings WHERE guid = @char", counts.settings + 1));
    // every inventory row points at an item of this character, every bag at an item of this character
    line(format!(
        "DO IF((SELECT COUNT(*) FROM acore_characters.character_inventory ci LEFT JOIN acore_characters.item_instance ii ON ii.guid = ci.item AND ii.owner_guid = @char WHERE ci.guid = @char AND ii.guid IS NULL) = 0, 1, {FAIL});"
    ));
    line(format!(
        "DO IF((SELECT COUNT(*) FROM acore_characters.character_inventory ci LEFT JOIN acore_characters.character_inventory b ON b.item = ci.bag AND b.guid = @char WHERE ci.guid = @char AND ci.bag <> 0 AND b.item IS NULL) = 0, 1, {FAIL});"
    ));

    line("SELECT '#R:alloc', @char, @item_base, @pet_base, @rename, CONCAT('x', HEX(@final_name));".into());
    line("COMMIT;".into());
    line("SELECT '#R:committed';".into());
    line(format!("DO RELEASE_LOCK('{IMPORT_LOCK}');"));

    Ok(ImportPlan { script: s, item_ids: model.items.iter().map(|i| i.id).collect(), pet_ids: model.pets.iter().map(|p| p.id).collect(), marker_data: marker, counts, not_applied_settings: not_applied })
}

/// What the realm answered after a successful import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    pub local_guid: u32,
    pub item_base: u32,
    pub pet_base: u32,
    pub renamed: bool,
    pub final_name: String,
}

/// Parse the answer of the script. `None` for "committed" means the script did not get as far as the commit report.
pub fn parse_report(output: &str) -> Result<(Allocation, bool)> {
    let mut alloc = None;
    let mut committed = false;
    for line in output.lines().filter(|l| !l.is_empty()) {
        let cells: Vec<&str> = line.split('\t').collect();
        match cells[0] {
            "#R:alloc" if cells.len() == 6 => {
                let num = |i: usize| cells[i].parse::<u64>().map_err(|_| PortableError::CorruptSnapshot(format!("import report: {:?} is not a number", cells[i])));
                let name_hex = cells[5].strip_prefix('x').ok_or_else(|| PortableError::CorruptSnapshot("import report: name is not hex".into()))?;
                let name = String::from_utf8(hex::decode(name_hex).map_err(|_| PortableError::CorruptSnapshot("import report: invalid hex".into()))?)
                    .map_err(|_| PortableError::CorruptSnapshot("import report: name is not UTF-8".into()))?;
                alloc = Some(Allocation {
                    local_guid: to_u32("guid", num(1)?)?,
                    item_base: to_u32("item base", num(2)?)?,
                    pet_base: to_u32("pet base", num(3)?)?,
                    renamed: num(4)? != 0,
                    final_name: name,
                });
            }
            "#R:committed" => committed = true,
            other => return Err(PortableError::CorruptSnapshot(format!("unexpected line in the import report: {other:?}"))),
        }
    }
    let alloc = alloc.ok_or_else(|| PortableError::CorruptSnapshot("the import report has no allocation".into()))?;
    Ok((alloc, committed))
}
