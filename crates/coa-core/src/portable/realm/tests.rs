//! Exporter tests on **recorded answers of a real database** (`testdata/golden/`, recorded by `live.rs` from the
//! synthetic characters of `testdata/realm-fixture.sql`). They need no database. `live.rs` re-records and verifies
//! the same answers against a real disposable MySQL.

use std::collections::HashMap;

use super::blockers::Blocker;
use super::export::{build, ExportRequest, Exported};
use super::script::{self, RawExport, SchemaProbe};
use super::*;
use crate::portable::ids::{CharacterId, ContentId, PortableItemId};
use crate::portable::model::*;
use crate::portable::snapshot;
use crate::portable::store::{ItemObservation, MappingState, Store};

const PROBE: &str = include_str!("testdata/golden/probe.txt");
const LIST: &str = include_str!("testdata/golden/list.txt");

fn answer(guid: u32) -> &'static str {
    match guid {
        1001 => include_str!("testdata/golden/export-1001.txt"),
        1002 => include_str!("testdata/golden/export-1002.txt"),
        1003 => include_str!("testdata/golden/export-1003.txt"),
        1004 => include_str!("testdata/golden/export-1004.txt"),
        1005 => include_str!("testdata/golden/export-1005.txt"),
        1006 => include_str!("testdata/golden/export-1006.txt"),
        1007 => include_str!("testdata/golden/export-1007.txt"),
        1008 => include_str!("testdata/golden/export-1008.txt"),
        1009 => include_str!("testdata/golden/export-1009.txt"),
        1010 => include_str!("testdata/golden/export-1010.txt"),
        1011 => include_str!("testdata/golden/export-1011.txt"),
        1012 => include_str!("testdata/golden/export-1012.txt"),
        other => panic!("no recorded answer for {other}"),
    }
}

fn probe() -> SchemaProbe {
    script::parse_probe(PROBE).unwrap()
}

fn raw(guid: u32) -> RawExport {
    script::parse_output(answer(guid), &script::queries(guid, &probe()).unwrap()).unwrap()
}

fn export_as(guid: u32, ruleset: Ruleset, prior: &HashMap<u32, (PortableItemId, String)>) -> Result<Exported> {
    build(&raw(guid), &ExportRequest { ruleset, local_guid: guid, character_id: None, prior_items: prior, prior_pets: &HashMap::new(), allow_online_session: false })
}

fn export(guid: u32) -> Exported {
    export_as(guid, Ruleset::Coa, &HashMap::new()).unwrap_or_else(|e| panic!("export of {guid} failed: {e}"))
}

fn roundtrip(model: &PortableCharacter) {
    let encoded = snapshot::encode(model).unwrap();
    let back = snapshot::decode(&encoded.payload, Some(&encoded.content_hash)).unwrap();
    assert_eq!(&back, model, "DB -> snapshot -> serialize -> deserialize must be structurally equal");
}

fn blockers_of(guid: u32) -> Vec<Blocker> {
    match export_as(guid, Ruleset::Coa, &HashMap::new()) {
        Err(PortableError::NotExportable(b)) => b,
        other => panic!("{guid} should be refused, got {:?}", other.map(|e| e.model.identity.name)),
    }
}

fn item_at(model: &PortableCharacter, container: Option<PortableItemId>, slot: u8) -> &PortableItem {
    model.items.iter().find(|i| i.container == container && i.slot == slot).unwrap_or_else(|| panic!("no item at {container:?}/{slot}"))
}

// ---- the characters -----------------------------------------------------------------------------------------------

#[test]
fn a_naked_level_one_character() {
    let e = export(1001);
    let m = &e.model;
    assert_eq!((m.identity.name.as_str(), m.progression.level, m.progression.money), ("Naked", 1, 0));
    assert_eq!((m.identity.race.to_string().as_str(), m.identity.class.to_string().as_str()), ("coa:race:1", "coa:class:12"));
    assert_eq!(m.ruleset, Ruleset::Coa);
    assert!(m.items.is_empty() && m.build.spells.is_empty() && m.pets.is_empty() && m.settings.is_empty() && m.extensions.is_empty());
    assert!(e.observations.is_empty() && e.warnings.is_empty());
    roundtrip(m);
}

#[test]
fn a_geared_character_with_bags_enchants_gems_and_special_items() {
    let e = export(1002);
    let m = &e.model;
    assert_eq!(m.identity.name, "Geared");
    assert_eq!((m.identity.gender, m.progression.level, m.progression.xp, m.progression.money), (1, 80, 12345, 200_060_523));
    assert_eq!(m.identity.class.to_string(), "coa:class:21");
    assert_eq!(m.identity.cosmetic_flags, 0x400, "of playerFlags 0x404 only the cosmetic bit is carried");
    assert_eq!((m.progression.honor.arena_points, m.progression.honor.total_honor, m.progression.honor.total_kills), (150, 4092, 777));
    assert_eq!((m.progression.known_currencies, m.progression.chosen_title, m.progression.watched_faction), (165, 5, 72));
    assert_eq!((m.progression.bank_slots, m.progression.stable_slots, m.progression.action_bars_mask, m.build.extra_bonus_talent_count), (4, 2, 15, 2));
    assert_eq!(m.progression.explored_zones.split_whitespace().count(), 128);
    assert_eq!(m.build.spells.len(), 60);
    assert!(m.build.spells.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(m.reputation.len(), 20);

    // 19 equipped + 4 bags + 24 in bags + 8 backpack + book + 2 bank + currency + gift = 60;
    // the three rows the realm would clean up itself are left out, and said so
    assert_eq!(m.items.len(), 60);
    assert_eq!(e.observations.len(), 60);
    assert_eq!(e.warnings.len(), 3, "{:?}", e.warnings);
    assert!(e.warnings.iter().any(|w| w.contains("missing from item_instance")));
    assert!(e.warnings.iter().any(|w| w.contains("container 29999")));
    assert!(e.warnings.iter().any(|w| w.contains("buyback")));

    // enchantments and gems keep their slots (permanent = 0, sockets 2..4)
    let chest = item_at(m, None, 3);
    assert_eq!(chest.entry.to_string(), format!("coa:item:{}", 30000 + 3 * 17));
    assert_eq!(
        chest.enchantments,
        vec![
            Enchantment { slot: 0, id: 3003, duration: 0, charges: 0 },
            Enchantment { slot: 2, id: 3878, duration: 0, charges: 0 },
            Enchantment { slot: 3, id: 3879, duration: 0, charges: 0 },
            Enchantment { slot: 4, id: 3880, duration: 0, charges: 0 },
        ]
    );
    assert!(item_at(m, None, 4).enchantments.is_empty());
    assert_eq!(item_at(m, None, 5).random_property_id, -57, "a suffix stays negative");
    assert_eq!(item_at(m, None, 4).creator_name.as_deref(), Some("Naked"), "the crafter is carried by name, never by guid");
    assert_eq!(item_at(m, None, 7).durability, 97);

    // bags and their contents
    let bag = item_at(m, None, 19);
    assert_eq!(bag.entry.to_string(), "coa:item:41019");
    let in_bag: Vec<&PortableItem> = m.items.iter().filter(|i| i.container == Some(bag.id)).collect();
    assert_eq!(in_bag.len(), 6);
    assert_eq!((item_at(m, Some(bag.id), 2).count, item_at(m, Some(bag.id), 1).count), (9, 5));
    assert!(m.items.iter().filter(|i| i.container.is_some()).all(|i| m.items.iter().any(|b| Some(b.id) == i.container && b.container.is_none())));

    // item state outside the plain columns
    let book = item_at(m, None, 39);
    assert_eq!(book.text.as_deref(), Some("A book.\nSecond line\twith a tab, a \"quote\", an 'apostrophe', a \\ backslash and \u{4e2d}\u{6587}."));
    assert_eq!(item_at(m, None, 40).charges, vec![-1, 0, 0, 0, 0]);
    assert_eq!(item_at(m, None, 40).duration, 3600);
    assert!(item_at(m, None, 41).charges.is_empty(), "NULL charges");
    let token = item_at(m, None, 118);
    assert_eq!((token.entry.to_string().as_str(), token.count), ("coa:item:375250", 12), "currency tokens ride in their slot");
    let wrapped = item_at(m, None, 42);
    assert_eq!(wrapped.gift, Some(Gift { entry: ContentId::new("coa", "item", 8000).unwrap(), flags: 8 }));

    roundtrip(m);
    m.validate().unwrap();
}

#[test]
fn quests_reputation_and_action_bars() {
    let m = export(1003).model;
    assert_eq!(m.quests.active.len(), 2);
    assert_eq!(m.quests.active[0], ActiveQuest { quest: 12001, status: 3, explored: false, timer: 0, mob_counts: [3, 0, 0, 0], item_counts: [0; 6], player_count: 0 });
    assert_eq!(m.quests.active[1], ActiveQuest { quest: 12002, status: 1, explored: true, timer: 600, mob_counts: [0; 4], item_counts: [2, 0, 0, 0, 0, 0], player_count: 0 });
    assert_eq!(m.quests.rewarded, vec![12000, 12003, 12004], "a rewarded row with active = 0 is not a completed quest to the realm either");
    assert_eq!(m.reputation.len(), 10);
    assert_eq!(m.reputation[3], ReputationEntry { faction: 4, standing: 200, flags: 1 });
    assert_eq!(m.actions.len(), 17);
    assert_eq!(m.actions.iter().filter(|a| a.spec == 1).count(), 5);
    assert!(m.actions.contains(&ActionButton { spec: 1, button: 4, action: 7, kind: 64 }), "macro buttons keep their type");
    assert_eq!(m.progression.level, 40);
    roundtrip(&m);
}

#[test]
fn spells_talents_glyphs_skills_settings_and_macros() {
    let e = export(1004);
    let m = &e.model;
    assert_eq!(m.identity.name, "\u{414}\u{440}\u{430}\u{43a}\u{43e}\u{43d}");
    assert_eq!(m.build.spells.len(), 300);
    assert_eq!(m.build.spells[0], (500_003, 2));
    assert_eq!(m.build.talents.len(), 5);
    assert_eq!(m.build.glyphs, vec![GlyphSet { talent_group: 0, glyphs: [1, 2, 3, 4, 5, 6] }, GlyphSet { talent_group: 1, glyphs: [7, 0, 0, 0, 0, 0] }]);
    assert_eq!(m.build.skills.len(), 10);
    assert_eq!(m.build.skills[0], Skill { skill: 100, value: 300, max: 450 });

    // CoA gameplay state: carried
    assert_eq!(m.settings["core.ascension_active_spec"], vec![54]);
    assert_eq!(m.settings["core.ascension_starter"], vec![1]);
    assert_eq!(m.settings["core.ascension_build.54"], vec![3, 56710, 56320, 90101]);
    assert_eq!(m.settings["core.ascension_bar.54"], vec![2, 0, 500_003, 1, 500_006]);
    assert_eq!(m.settings["core.ascension_slot.active"], vec![0]);
    // bots: dropped; nonessential and unknown state is quarantined: kept aside, never applied (policy v2)
    assert!(!m.settings.contains_key("coa.bot.gear"));
    for aside in ["coa.gear_pref_weapon", "core.spell_charge.804197", "mod.example.new_feature", "core.wildcard.cards"] {
        assert!(!m.settings.contains_key(aside), "{aside} must not be carried");
    }
    let ext = &m.extensions["coa:unlisted-settings"];
    assert!(ext.is_intact());
    let aside: std::collections::BTreeMap<String, Vec<u32>> = serde_json::from_slice(&ext.payload.0).unwrap();
    assert_eq!(aside.keys().map(String::as_str).collect::<Vec<_>>(), vec!["coa.gear_pref_weapon", "core.spell_charge.804197", "core.wildcard.cards", "mod.example.new_feature"]);
    assert_eq!(aside["core.spell_charge.804197"], vec![2, 1_790_000_000], "the values are kept exactly");
    assert_eq!(m.settings.keys().map(String::as_str).collect::<Vec<_>>(), vec!["core.ascension_active_spec", "core.ascension_bar.54", "core.ascension_build.54", "core.ascension_slot.active", "core.ascension_starter"]);
    assert_eq!(aside["mod.example.new_feature"], vec![9, 8, 7]);
    assert!(e.warnings.iter().any(|w| w.contains("kept aside")));

    // macros are carried as the binary blob they are; the other client data is not
    assert_eq!(m.client_data.keys().copied().collect::<Vec<_>>(), vec![5]);
    let macros = &m.client_data[&5];
    assert_eq!(macros.time, 1_790_000_000);
    assert_eq!(macros.data.0.len(), 607);
    assert!(macros.data.0.ends_with(b"\x00\x01\xff end"));
    roundtrip(m);
}

#[test]
fn hunter_and_summoned_pets() {
    let m = export(1005).model;
    assert_eq!(m.pets.len(), 3);
    let [current, stabled, summon] = [&m.pets[0], &m.pets[1], &m.pets[2]];
    assert_eq!((current.name.as_str(), current.slot, current.pet_type, current.level), ("Rex", 0, 1, 60));
    assert_eq!((current.entry.to_string().as_str(), current.model_id, current.created_by_spell, current.health), ("coa:creature:42", 901, 1515, 5000));
    assert_eq!(current.spells.len(), 6);
    assert_eq!(current.spells[0], PetSpell { spell: 16827, active: 0 });
    assert_eq!(current.action_bar, "7 2 0 0 1 0 1 1 0 ");
    assert_eq!((stabled.name.as_str(), stabled.slot, stabled.renamed), ("Fang", 1, true));
    let declined = stabled.declined_names.as_ref().unwrap();
    assert_eq!(declined[0], "\u{424}\u{430}\u{43d}\u{433}\u{430}");
    assert!(current.declined_names.is_none());
    assert_eq!((summon.name.as_str(), summon.slot, summon.pet_type), ("Imp", 100, 0));
    assert_eq!(summon.spells, vec![PetSpell { spell: 3110, active: 1 }]);
    assert_eq!(summon.action_bar, "", "a NULL action bar is an empty one");
    assert_eq!(m.progression.stable_slots, 2);
    let ids: std::collections::HashSet<_> = m.pets.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), 3, "every pet has its own portable id");
    roundtrip(&m);
}

// ---- who may be exported ------------------------------------------------------------------------------------------------

#[test]
fn offline_challenge_free_characters_are_exportable() {
    for guid in [1001, 1002, 1003, 1004, 1005, 1012] {
        assert!(super::export::blockers(&raw(guid)).unwrap().is_empty(), "{guid}");
    }
    // history is not an active state: a finished challenge, game mode 0 and condition flags do not block
    assert_eq!(export(1012).model.identity.name, "History");
}

#[test]
fn every_blocking_state_refuses_the_export() {
    assert_eq!(blockers_of(1006), vec![Blocker::Online]);
    assert_eq!(blockers_of(1007), vec![Blocker::ActiveChallenge, Blocker::ActiveGameMode]);
    assert_eq!(blockers_of(1008), vec![Blocker::PendingManastormCaches]);
    assert_eq!(blockers_of(1009), vec![Blocker::Deleted]);
    assert_eq!(blockers_of(1010), vec![Blocker::BotAccount]);
    assert_eq!(blockers_of(1011), vec![Blocker::UnclassifiedState("mod_unknown_state".into())]);
    assert!(blockers_of(1007).iter().all(Blocker::is_challenge_state), "decision D6: challenge, game mode and Manastorm state block transfer");
    let message = PortableError::NotExportable(blockers_of(1007)).to_string();
    assert!(message.contains("challenge") && message.contains("game mode"), "{message}");
}

#[test]
fn the_character_list_agrees_with_the_export() {
    let list = parse_listing(LIST).unwrap();
    assert_eq!(list.len(), 11, "twelve fixture characters, minus the bot");
    let find = |g: u32| list.iter().find(|c| c.local_guid == g).unwrap();
    assert_eq!((find(1002).name.as_str(), find(1002).level, find(1002).class, find(1002).race), ("Geared", 80, 21, 2));
    assert_eq!(find(1004).name, "\u{414}\u{440}\u{430}\u{43a}\u{43e}\u{43d}");
    assert!(find(1001).eligible() && find(1012).eligible());
    assert_eq!(find(1006).blockers, vec![Blocker::Online]);
    assert_eq!(find(1007).blockers, vec![Blocker::ActiveChallenge, Blocker::ActiveGameMode]);
    assert_eq!(find(1008).blockers, vec![Blocker::PendingManastormCaches]);
    assert_eq!(find(1009).blockers, vec![Blocker::Deleted]);
    // the listing is cheap and cannot see unknown tables; the export refuses 1011
    assert!(find(1011).eligible());
    assert!(list.iter().all(|c| c.local_guid != 1010), "bots are not even listed");
    assert!(parse_listing("1\tx6162\t1\t1").is_err());
    assert!(parse_listing(&LIST.replace("x476561726564", "xZZ")).is_err());
}

#[test]
fn the_schema_probe_finds_the_unknown_per_character_table() {
    let probe = probe();
    assert!(probe.missing_required().is_empty());
    assert_eq!(probe.unclassified_character_tables(), vec![("mod_unknown_state".to_string(), "guid".to_string())]);
    let names: Vec<String> = script::queries(1, &probe).unwrap().into_iter().map(|q| q.name).collect();
    for expected in ["block:challenge", "block:gamemode", "block:trial", "block:manastorm", "unclassified:mod_unknown_state"] {
        assert!(names.contains(&expected.to_string()), "{expected}");
    }
    // a realm that lacks a core table cannot be exported from
    let mut broken = probe.clone();
    broken.tables.remove("item_instance");
    assert_eq!(broken.missing_required(), vec!["item_instance"]);
}

// ---- identity of items, rulesets, robustness ---------------------------------------------------------------------------

#[test]
fn items_keep_their_portable_ids_but_a_recycled_guid_gets_a_new_one() {
    let first = export(1002);
    let prior: HashMap<u32, (PortableItemId, String)> = first.observations.iter().map(|o| (o.local_item_guid, (o.portable_item_id, o.identity.clone()))).collect();
    let again = export_as(1002, Ruleset::Coa, &prior).unwrap();
    let ids = |e: &Exported| e.model.items.iter().map(|i| i.id).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids(&first), ids(&again), "unchanged items keep their ids");
    assert_eq!(ids(&first).len(), 60, "every item has its own id");

    // the realm recycled guid 20001: the mapping was made for a different item, so the item there now is a new one
    let mut recycled = prior.clone();
    let (old_id, _) = recycled[&20001].clone();
    recycled.insert(20001, (old_id, "v1:some-other-item".to_string()));
    let after = export_as(1002, Ruleset::Coa, &recycled).unwrap();
    let obs = after.observations.iter().find(|o| o.local_item_guid == 20001).unwrap();
    assert_ne!(obs.portable_item_id, old_id, "a recycled guid must not inherit the old item's identity");
    assert_eq!(after.observations.iter().filter(|o| o.local_item_guid != 20001).filter(|o| prior[&o.local_item_guid].0 == o.portable_item_id).count(), 59);

    // a mapping for a guid the realm no longer has is simply not used
    let mut stale = prior;
    stale.insert(99_999_999, (PortableItemId::new(), "v1:x".into()));
    assert_eq!(export_as(1002, Ruleset::Coa, &stale).unwrap().observations.len(), 60);
}

#[test]
fn a_reexport_keeps_the_portable_character_id() {
    let id = CharacterId::new();
    let e = build(&raw(1002), &ExportRequest { ruleset: Ruleset::Coa, local_guid: 1002, character_id: Some(id), prior_items: &HashMap::new(), prior_pets: &HashMap::new(), allow_online_session: false }).unwrap();
    assert_eq!(e.model.character_id, id);
}

#[test]
fn the_same_realm_data_gives_the_same_canonical_bytes() {
    let prior = HashMap::new();
    let a = export_as(1004, Ruleset::Coa, &prior).unwrap().model;
    let mut b = export_as(1004, Ruleset::Coa, &prior).unwrap().model;
    // everything that is generated (ids) is aligned; everything that comes from the realm must be identical
    b.character_id = a.character_id;
    for (pa, pb) in a.pets.iter().zip(b.pets.iter_mut()) {
        pb.id = pa.id;
    }
    assert_eq!(snapshot::content_hash(&a).unwrap(), snapshot::content_hash(&b).unwrap());
}

#[test]
fn the_wildcard_ruleset_uses_its_own_namespace_and_quarantines_wildcard_state() {
    let coa = export_as(1004, Ruleset::Coa, &HashMap::new()).unwrap().model;
    let wildcard = export_as(1004, Ruleset::Wildcard, &HashMap::new()).unwrap().model;
    assert_eq!(wildcard.ruleset, Ruleset::Wildcard);
    assert_eq!(wildcard.content_namespace, "wildcard");
    assert_eq!(wildcard.identity.class.to_string(), "wildcard:class:25");
    // Wildcard keys are not on the carry list on any ruleset (policy v2); they are kept, not applied
    for m in [&coa, &wildcard] {
        assert!(!m.settings.contains_key("core.wildcard.cards"));
        assert!(m.extensions["coa:unlisted-settings"].is_intact());
    }
    assert_eq!(wildcard.settings, coa.settings);
    // the realm profile of a database decides the ruleset, not the caller
    let wc_db = Db::with_tools(std::path::PathBuf::new(), 1, "root", "x", Mode::Wildcard);
    let coa_db = Db::with_tools(std::path::PathBuf::new(), 1, "root", "x", Mode::Coa);
    assert_eq!((ruleset_of(&wc_db), ruleset_of(&coa_db)), (Ruleset::Wildcard, Ruleset::Coa));
    assert_eq!(wc_db.realm_schema(script::CHARACTERS_SCHEMA), "acore_characters_wildcard");
}

#[test]
fn windows_line_endings_do_not_matter() {
    let crlf = answer(1002).replace("\r\n", "\n").replace('\n', "\r\n");
    let queries = script::queries(1002, &probe()).unwrap();
    let parsed = script::parse_output(&crlf, &queries).unwrap();
    let e = build(&parsed, &ExportRequest { ruleset: Ruleset::Coa, local_guid: 1002, character_id: None, prior_items: &HashMap::new(), prior_pets: &HashMap::new(), allow_online_session: false }).unwrap();
    assert_eq!((e.model.items.len(), e.model.identity.name.as_str()), (60, "Geared"));
}

/// Whatever a damaged or hostile answer looks like, the exporter returns an error or a valid model, never panics.
#[test]
fn damaged_answers_never_panic() {
    let queries = script::queries(1002, &probe()).unwrap();
    let original = answer(1002).as_bytes().to_vec();
    let mut x = 0x2545_F491u32;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x
    };
    let (mut errors, mut oks) = (0, 0);
    for _ in 0..400 {
        let mut bytes = original.clone();
        for _ in 0..1 + next() % 4 {
            let at = next() as usize % bytes.len();
            bytes[at] = [b'Z', b'\t', b'\n', b'x', b'-', b'9', b'0', b'~'][next() as usize % 8];
        }
        let Ok(text) = String::from_utf8(bytes) else { continue };
        match script::parse_output(&text, &queries).and_then(|raw| build(&raw, &ExportRequest { ruleset: Ruleset::Coa, local_guid: 1002, character_id: None, prior_items: &HashMap::new(), prior_pets: &HashMap::new(), allow_online_session: false })) {
            Ok(e) => {
                e.model.validate().unwrap();
                oks += 1;
            }
            Err(_) => errors += 1,
        }
    }
    assert!(errors > 0, "some of the damage must be detected");
    assert_eq!(oks + errors, 400);
}

#[test]
fn realm_errors_are_told_apart() {
    let schema = realm_error(crate::Error::Invalid("database command failed: ERROR 1054 (42S22) at line 5: Unknown column 'c.nope' in 'field list'".into()));
    assert!(matches!(schema, PortableError::SchemaMismatch(_)));
    let down = realm_error(crate::Error::Invalid("The database is not running. Start the server first.".into()));
    assert!(matches!(down, PortableError::RealmRead(_)));
}

// ---- registering in the local store ----------------------------------------------------------------------------------------

#[test]
fn make_portable_registers_everything_in_one_step_and_refuses_twice() {
    let mut store = Store::open_in_memory().unwrap();
    let profile = store.default_profile().unwrap();
    let exported = export(1002);
    let expected_model = exported.model.clone();
    let observed = exported.observations.len();
    let made = register(&mut store, profile, "realm-a", exported).unwrap();
    assert_eq!(made.revision, 1);
    assert_eq!(made.warnings.len(), 3);

    let record = store.character(made.character_id).unwrap();
    assert_eq!((record.revision, record.name.as_str(), record.level), (1, "Geared", 80));
    assert_eq!(store.load_current(made.character_id).unwrap(), expected_model);
    assert_eq!(store.find_by_local("realm-a", 1002).unwrap(), Some(made.character_id));
    let mappings = store.server_mappings(made.character_id).unwrap();
    assert_eq!((mappings.len(), mappings[0].local_guid, mappings[0].last_revision), (1, 1002, 1));
    assert_eq!(mappings[0].state, MappingState::Synced);
    assert_eq!(store.item_mappings(made.character_id, "realm-a").unwrap().len(), observed);

    // the same local character cannot be made portable twice
    let again = register(&mut store, profile, "realm-a", export(1002));
    assert!(matches!(again, Err(PortableError::AlreadyPortable { local_guid: 1002, character, .. }) if character == made.character_id), "{again:?}");
    assert_eq!(store.list_characters(profile).unwrap().len(), 1);

    // a re-export with the store's mappings keeps every item id, so the next revision is a plain update
    let lookup = store.active_item_lookup(made.character_id, "realm-a").unwrap();
    let re = build(&raw(1002), &ExportRequest { ruleset: Ruleset::Coa, local_guid: 1002, character_id: Some(made.character_id), prior_items: &lookup, prior_pets: &HashMap::new(), allow_online_session: false }).unwrap();
    assert_eq!(re.model, expected_model);
    let revision = store.commit_snapshot(made.character_id, 1, re.model, "realm-a", Some("re-export")).unwrap();
    let report = store.reconcile_item_mappings(made.character_id, "realm-a", revision, &re.observations).unwrap();
    assert_eq!((report.confirmed, report.added, report.guid_reused, report.moved, report.absent), (observed, 0, 0, 0, 0));
}

#[test]
fn a_failed_registration_leaves_nothing_behind() {
    let mut store = Store::open_in_memory().unwrap();
    let profile = store.default_profile().unwrap();
    let mut exported = export(1002);
    // two observations for one local guid: the mapping step fails after the character was already inserted
    let dup = exported.observations[0].clone();
    exported.observations.push(ItemObservation { portable_item_id: PortableItemId::new(), ..dup });
    assert!(register(&mut store, profile, "realm-a", exported).is_err());
    assert!(store.list_characters(profile).unwrap().is_empty(), "no half-registered character");
    assert_eq!(store.find_by_local("realm-a", 1002).unwrap(), None);
}
