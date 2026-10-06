//! Tests of the import plan and its SQL, without a database. The behaviour against a real MySQL is in `live.rs`.

use std::collections::BTreeSet;

use super::plan::*;
use super::policy::IMPORT_MARKER_SOURCE;
use super::script::SchemaProbe;
use super::sqlenc::Val;
use crate::portable::fixtures::geared_level_eighty;
use crate::portable::ids::{ContentId, PortableItemId, PortablePetId};
use crate::portable::model::*;
use crate::portable::PortableError;

fn probe_with(tables: &[&str]) -> SchemaProbe {
    SchemaProbe { tables: tables.iter().map(|t| t.to_string()).collect(), character_columns: Default::default() }
}

fn ctx<'a>(probe: &'a SchemaProbe, users: &'a [String]) -> PlanContext<'a> {
    PlanContext { ruleset: Ruleset::Coa, account: 5001, revision: 3, nonce: [11, 22, 33, 44], max_characters_per_account: 10, game_server_users: users, probe, session: None, knowledge: None }
}

fn users() -> Vec<String> {
    vec!["acore".to_string()]
}

fn all_tables() -> SchemaProbe {
    let mut names: Vec<&str> = CHARACTER_KEYED.iter().map(|(t, _)| *t).collect();
    names.extend(ITEM_KEYED.iter().map(|(t, _)| *t));
    names.extend(PET_KEYED.iter().map(|(t, _)| *t));
    names.push("reserved_name");
    probe_with(&names)
}

fn plan_of(model: &PortableCharacter) -> ImportPlan {
    let u = users();
    build_plan(model, &ctx(&all_tables(), &u)).unwrap()
}

/// The script with every hex literal replaced by `H`: what is left is the fixed scaffold.
fn skeleton(script: &str) -> String {
    let bytes = script.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'0' && bytes.get(i + 1) == Some(&b'x') && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit) && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            i += 2;
            while i < bytes.len() && bytes[i].is_ascii_hexdigit() {
                i += 1;
            }
            out.push('H');
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn hostile_model() -> PortableCharacter {
    let mut m = geared_level_eighty();
    m.identity.name = "Bob');DROP TABLE x;--".into();
    m.items[0].text = Some("\"; DELETE FROM item_instance; -- \\ \0 \n\t' OR '1'='1".into());
    m.client_data.get_mut(&5).unwrap().data = Bytes(b"'; DROP DATABASE acore_characters; -- \x00\xff".to_vec());
    m.settings.insert("core.ascension_build.77".into(), vec![1, 2, 3]);
    m.pets.push(PortablePet {
        id: PortablePetId::new(),
        entry: ContentId::new("coa", "creature", 416).unwrap(),
        model_id: 1,
        created_by_spell: 2,
        pet_type: 0,
        level: 1,
        exp: 0,
        react_state: 1,
        name: "p');DROP TABLE x;--".into(),
        renamed: false,
        slot: 100,
        health: 1,
        mana: 1,
        happiness: 1,
        action_bar: "'; DROP TABLE pet_spell; --".into(),
        spells: vec![PetSpell { spell: 3110, active: 1 }],
        declined_names: Some(["a'".into(), "b\"".into(), "c\\".into(), "d;".into(), "e--".into()]),
    });
    m.normalized()
}

// ---- the script ---------------------------------------------------------------------------------------------------

#[test]
fn the_script_is_exactly_one_realm_transaction() {
    let plan = plan_of(&geared_level_eighty());
    let s = &plan.script;
    assert_eq!(s.matches("START TRANSACTION;").count(), 1);
    assert_eq!(s.matches("COMMIT;").count(), 1);
    assert!(s.find("START TRANSACTION;").unwrap() < s.find("INSERT INTO").unwrap());
    assert!(s.find("COMMIT;").unwrap() > s.rfind("INSERT INTO").unwrap(), "every insert is before the commit");
    // nothing is written after the commit, and nothing implicitly commits before it
    let after = &s[s.find("COMMIT;").unwrap()..];
    assert!(!after.contains("INSERT") && !after.contains("DELETE") && !after.contains("UPDATE"));
    let upper = s.to_ascii_uppercase();
    for forbidden in ["CREATE ", "ALTER ", "DROP ", "TRUNCATE ", "RENAME TABLE", "LOCK TABLES", "UNLOCK TABLES", "SET AUTOCOMMIT", "SET GLOBAL", "LOAD DATA", "OUTFILE", "GRANT ", "ROLLBACK"] {
        assert!(!upper.contains(forbidden), "the script contains {forbidden:?}");
    }
    // the transaction proves its own result before it commits
    let before_commit = &s[..s.find("COMMIT;").unwrap()];
    assert!(before_commit.matches("DO IF(").count() >= 20, "assertions: {}", before_commit.matches("DO IF(").count());
    assert!(before_commit.contains("(SELECT 1 UNION ALL SELECT 2)"), "a failing assertion raises an SQL error");
    assert!(s.starts_with("DO IF(GET_LOCK('coa_portable_import', 10) = 1"));
    assert!(s.trim_end().ends_with("DO RELEASE_LOCK('coa_portable_import');"));
}

#[test]
fn untrusted_text_and_blobs_exist_only_as_hex_literals() {
    let benign = plan_of(&geared_level_eighty());
    let hostile = plan_of(&hostile_model());
    // the scaffold differs only by the rows the hostile model adds (one pet): what the hostile data contains cannot change it
    let mut quiet = hostile_model();
    quiet.identity.name = "Quiet".into();
    quiet.items[0].text = Some("plain".into());
    quiet.client_data.get_mut(&5).unwrap().data = Bytes(vec![1, 2, 3]);
    quiet.pets[1].name = "pet".into();
    quiet.pets[1].action_bar = "bar".into();
    quiet.pets[1].declined_names = Some(["a".into(), "b".into(), "c".into(), "d".into(), "e".into()]);
    let quiet = plan_of(&quiet);
    let (a, b) = (skeleton(&hostile.script), skeleton(&quiet.script));
    if a != b {
        let at = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
        panic!("scaffolds differ at {at}: {:?} vs {:?}", &a[at.saturating_sub(60)..(at + 60).min(a.len())], &b[at.saturating_sub(60)..(at + 60).min(b.len())]);
    }
    assert_ne!(hostile.script, quiet.script);
    for fragment in ["DROP TABLE", "DELETE FROM item_instance;", "DROP DATABASE", "OR '1'='1", "Bob", "\\", "\0"] {
        assert!(!hostile.script.contains(fragment), "{fragment:?} leaked into the SQL");
    }
    assert!(benign.script.len() < hostile.script.len());
    // the only quotes in the whole script belong to fixed literals of this crate
    let quotes = |s: &str| skeleton(s).matches('\'').count();
    assert_eq!(quotes(&hostile.script), quotes(&quiet.script));
}

#[test]
fn every_text_of_the_character_is_in_the_script_as_its_exact_utf8_bytes() {
    let plan = plan_of(&hostile_model());
    for text in ["Bob');DROP TABLE x;--", "p');DROP TABLE x;--", "core.ascension_build.77", "1 2 3 "] {
        assert!(plan.script.contains(&Val::text(text).sql()), "{text:?}");
    }
    assert!(plan.script.contains(&Val::Bytes(b"'; DROP DATABASE acore_characters; -- \x00\xff".to_vec()).sql()));
}

#[test]
fn every_reference_is_remapped_through_the_realms_own_allocation() {
    let model = geared_level_eighty();
    let plan = plan_of(&model);
    let n = model.items.len();
    assert_eq!(plan.item_ids.len(), n);
    assert_eq!(plan.item_ids, model.items.iter().map(|i| i.id).collect::<Vec<_>>());
    // item i is @item_base + i, in the item rows and in the inventory rows; nothing outside 0..n
    let refs: BTreeSet<usize> = plan.script.match_indices("(@item_base + ").map(|(at, m)| plan.script[at + m.len()..].split(')').next().unwrap().parse().unwrap()).collect();
    assert_eq!(refs, (0..n).collect::<BTreeSet<_>>());
    // the script allocates from the realm, never from the canonical data
    assert!(plan.script.contains("SET @char := (SELECT IFNULL(MAX(guid), 0) + 1 FROM acore_characters.characters);"));
    assert!(plan.script.contains("SET @item_base := GREATEST(0, "));
    assert!(plan.script.contains("SET @pet_base := GREATEST(0, "));
    for table in ["item_instance", "mail_items", "auctionhouse", "guild_bank_item", "character_inventory"] {
        assert!(plan.script.contains(&format!("FROM acore_characters.`{table}`)")), "{table} must be considered when allocating item guids");
    }
    // the character row and every per-character row use @char; no realm guid of any other character is written
    let item_insert = plan.script.lines().find(|l| l.starts_with("INSERT INTO acore_characters.`item_instance`")).unwrap();
    assert!(item_insert.contains("VALUES\n") || plan.script.contains("`item_instance` (`guid`"));
    assert!(plan.script.contains("(@char, "), "per-character rows are keyed by the new guid");
    // bags: an item in a bag names the bag's plan index
    let bag_index = model.items.iter().position(|i| i.container.is_none() && i.slot == 19).unwrap();
    assert!(plan.script.contains(&format!("(@char, (@item_base + {bag_index}), 0,")), "an item in the first bag points at that bag, slot 0");
}

#[test]
fn the_plan_counts_what_it_writes_and_asserts_it() {
    let model = geared_level_eighty();
    let plan = plan_of(&model);
    let c = &plan.counts;
    assert_eq!((c.items, c.inventory, c.gifts), (model.items.len(), model.items.len(), 1));
    assert_eq!((c.spells, c.talents, c.skills, c.glyphs), (1284, 0, 23, 1));
    assert_eq!((c.reputation, c.quests, c.rewarded, c.actions, c.pets, c.macros), (105, 2, 4, 12, 1, 1));
    assert_eq!(c.settings, 4, "active spec, starter, build and bar are the carried keys of the fixture character");
    assert!(plan.script.contains(&format!("acore_characters.item_instance WHERE owner_guid = @char) = {},", model.items.len())));
    assert!(plan.script.contains("acore_characters.character_spell WHERE guid = @char) = 1284,"));
    assert!(plan.script.contains("acore_characters.character_settings WHERE guid = @char) = 5,"), "carried settings plus the marker");
    // every inventory row must resolve to an item of this character, every bag to an inventory row of this character
    assert!(plan.script.contains("ii.guid IS NULL) = 0"));
    assert!(plan.script.contains("b.item IS NULL) = 0"));
}

#[test]
fn only_the_carried_settings_are_written_and_the_rest_is_reported_as_not_applied() {
    let mut model = geared_level_eighty();
    model.settings.insert("core.spell_charge.804197".into(), vec![2, 1_790_000_000]);
    model.settings.insert("core.destiny_weaver".into(), vec![1]);
    model.settings.insert("core.wildcard.cards".into(), vec![1]);
    let plan = plan_of(&model.normalized());
    for carried in ["core.ascension_active_spec", "core.ascension_starter", "core.ascension_build.54", "core.ascension_bar.54"] {
        assert!(plan.script.contains(&Val::text(carried).sql()), "{carried}");
    }
    for skipped in ["core.spell_charge.804197", "core.destiny_weaver", "core.wildcard.cards"] {
        assert!(!plan.script.contains(&Val::text(skipped).sql()), "{skipped} must not reach the realm");
        assert!(plan.not_applied_settings.iter().any(|s| s == skipped), "{skipped}");
    }
    assert_eq!(plan.counts.settings, 4);
}

#[test]
fn the_marker_is_written_in_the_same_transaction_and_matches_the_journals() {
    let plan = plan_of(&geared_level_eighty());
    assert_eq!(plan.marker_data, "11 22 33 44 3 ");
    assert_eq!(plan.marker_data, crate::portable::store::marker_text([11, 22, 33, 44], 3), "plan and journal must agree on the marker");
    let marker_row = format!("(@char, {}, {})", Val::text(IMPORT_MARKER_SOURCE).sql(), Val::text("11 22 33 44 3 ").sql());
    let at = plan.script.find(&marker_row).expect("the marker row is inserted");
    assert!(at < plan.script.find("COMMIT;").unwrap());
}

#[test]
fn the_guards_prove_the_realm_is_stopped_and_the_account_is_usable() {
    let plan = plan_of(&geared_level_eighty());
    let s = &plan.script;
    assert!(s.contains("WHERE online <> 0) = 0"));
    assert!(s.contains(&format!("information_schema.processlist WHERE id <> CONNECTION_ID() AND user IN ({})", Val::text("acore").sql())));
    assert!(s.contains("acore_auth.account WHERE id = 5001) = 1"));
    assert!(s.contains("UPPER(username) NOT LIKE 'COABOT%'"));
    assert!(s.contains("WHERE account = 5001) < 10"));
    // the start position comes from the realm's world data, not from the snapshot
    assert!(s.contains("INTO @map, @zone, @x, @y, @z, @o FROM acore_world.playercreateinfo WHERE race = 1 AND class = 21;"));
    let none: Vec<String> = vec![];
    assert!(build_plan(&geared_level_eighty(), &ctx(&all_tables(), &none)).is_err(), "a plan without a game server user cannot prove the realm is stopped");
}

#[test]
fn names_are_checked_inside_the_transaction_and_renamed_when_taken() {
    let s = plan_of(&geared_level_eighty()).script;
    assert!(s.contains("SET @name := _utf8mb4 0x"));
    assert!(s.contains("name COLLATE utf8mb4_unicode_ci = @name COLLATE utf8mb4_unicode_ci"));
    assert!(s.contains("acore_characters.reserved_name"));
    assert!(s.contains("SET @final_name := IF(@rename = 1, CONCAT(LEFT(@name, 25 - CHAR_LENGTH(HEX(@char))), HEX(@char)), @name);"));
    assert!(s.contains("SET @at_login := IF(@rename = 1, 1, 0);"));
    let u = users();
    let without = build_plan(&geared_level_eighty(), &ctx(&probe_with(&[]), &u)).unwrap().script;
    assert!(!without.contains("reserved_name"), "tables the realm does not have are not touched");
}

#[test]
fn leftovers_of_a_deleted_character_with_the_same_guid_are_removed_first() {
    let plan = plan_of(&geared_level_eighty());
    let s = &plan.script;
    let first_insert = s.find("INSERT INTO").unwrap();
    for (table, column) in CHARACTER_KEYED {
        let delete = format!("DELETE FROM acore_characters.`{table}` WHERE `{column}` = @char;");
        let at = s.find(&delete).unwrap_or_else(|| panic!("no cleanup for {table}"));
        assert!(at < first_insert, "{table}");
    }
    // tables whose `guid` column is not a character guid must never be touched
    for never in ["groups", "creature_respawn", "gameobject_respawn", "guild", "mail", "instance"] {
        assert!(!s.contains(&format!("DELETE FROM acore_characters.`{never}`")), "{never}");
    }
    // only tables the realm has
    let u = users();
    let some = build_plan(&geared_level_eighty(), &ctx(&probe_with(&["characters", "item_instance", "character_inventory"]), &u)).unwrap().script;
    assert!(some.contains("DELETE FROM acore_characters.`item_instance` WHERE `owner_guid` = @char;"));
    assert!(!some.contains("DELETE FROM acore_characters.`coa_character_challenge`"));
}

// ---- what is refused ------------------------------------------------------------------------------------------------

#[test]
fn a_character_of_another_ruleset_or_namespace_cannot_be_planned() {
    let u = users();
    let p = all_tables();
    let wildcard = PlanContext { ruleset: Ruleset::Wildcard, ..ctx(&p, &u) };
    assert!(matches!(build_plan(&geared_level_eighty(), &wildcard), Err(PortableError::WrongRealm { .. })));
    let mut m = geared_level_eighty();
    m.items[0].entry = ContentId::new("mod:other", "item", 5).unwrap();
    assert!(matches!(build_plan(&m, &ctx(&p, &u)), Err(PortableError::Invalid(_))), "a mod item cannot be given a number in the realm's namespace");
    let mut m = geared_level_eighty();
    m.items[0].entry = ContentId::new("coa", "item", u32::MAX as u64 + 1).unwrap();
    assert!(build_plan(&m, &ctx(&p, &u)).is_err(), "entries must fit the realm's 32-bit column");
    let mut m = geared_level_eighty();
    m.identity.class = ContentId::new("coa", "race", 12).unwrap();
    assert!(build_plan(&m, &ctx(&p, &u)).is_err(), "a class must be a class");
}

#[test]
fn an_invalid_model_is_not_planned() {
    let mut m = geared_level_eighty();
    m.progression.money = u32::MAX;
    let u = users();
    assert!(build_plan(&m, &ctx(&all_tables(), &u)).is_err());
}

#[test]
fn an_empty_character_makes_a_small_valid_plan() {
    let m = crate::portable::fixtures::naked_level_one();
    let plan = plan_of(&m);
    assert_eq!(plan.counts, PlanCounts { settings: 0, ..PlanCounts::default() });
    assert!(plan.item_ids.is_empty() && plan.pet_ids.is_empty());
    assert!(!plan.script.contains("INSERT INTO acore_characters.`item_instance`"));
    assert!(plan.script.contains("INSERT INTO acore_characters.`characters`"));
    assert!(plan.script.contains("acore_characters.character_settings WHERE guid = @char) = 1,"), "only the marker");
}

#[test]
fn enchantments_charges_and_texts_are_rendered_the_way_the_realm_stores_them() {
    let model = geared_level_eighty();
    let chest = model.items.iter().find(|i| i.container.is_none() && i.slot == 3).unwrap();
    let text: String = {
        let mut slots = [[0u32; 3]; 12];
        for e in &chest.enchantments {
            slots[e.slot as usize] = [e.id, e.duration, e.charges];
        }
        slots.iter().flatten().map(|v| format!("{v} ")).collect()
    };
    assert!(text.starts_with("3003 0 0 0 0 0 3878 0 0 3879 0 0 0 0 0 "));
    assert_eq!(text.split_whitespace().count(), 36);
    let plan = plan_of(&model);
    assert!(plan.script.contains(&Val::text(text).sql()));
    // NULL charges stay NULL, real ones are five numbers
    let mut m = geared_level_eighty();
    m.items[1].charges = vec![-1, 2];
    let plan = plan_of(&m.normalized());
    assert!(plan.script.contains(&Val::text("-1 2 0 0 0 ").sql()));
}

// ---- the realm's report -------------------------------------------------------------------------------------------------

#[test]
fn the_realms_report_is_parsed_strictly() {
    let name_hex = hex::encode("Geared");
    let ok = format!("#R:alloc\t1013\t20064\t3004\t0\tx{name_hex}\n#R:committed\n");
    let (alloc, committed) = parse_report(&ok).unwrap();
    assert!(committed);
    assert_eq!(alloc, Allocation { local_guid: 1013, item_base: 20064, pet_base: 3004, renamed: false, final_name: "Geared".into() });
    let (_, committed) = parse_report(&format!("#R:alloc\t1\t2\t3\t1\tx{name_hex}\n")).unwrap();
    assert!(!committed, "no commit line: the commit is not proven");
    assert!(parse_report("").is_err());
    assert!(parse_report("#R:committed\n").is_err(), "a commit without an allocation");
    assert!(parse_report("#R:alloc\t1\t2\t3\n").is_err());
    assert!(parse_report("#R:alloc\tx\t2\t3\t0\txab\n").is_err());
    assert!(parse_report("#R:alloc\t4294967296\t2\t3\t0\txab\n").is_err());
    assert!(parse_report("#R:surprise\n").is_err());
    let _ = (PortableItemId::new(), BTreeSet::<u8>::new());
}

#[test]
fn stock_talents_are_never_written_because_a_bad_row_kills_the_realm() {
    let mut m = geared_level_eighty();
    m.build.talents = vec![(900_001, 1), (900_002, 1)];
    let plan = plan_of(&m.normalized());
    assert!(!plan.script.contains("`character_talent` ("), "no INSERT into character_talent");
    assert_eq!(plan.counts.talents, 0);
    assert!(plan.not_applied_settings.iter().any(|s| s.starts_with("character_talent (2 stock talent rows")), "{:?}", plan.not_applied_settings);
    // and the transaction asserts that the character has none, so a leftover row of an earlier character cannot survive either
    assert!(plan.script.contains("acore_characters.character_talent WHERE guid = @char) = 0,"));
}

// ---- Phase 6: selected appearances ----------------------------------------------------------------------------------------

#[test]
fn selected_appearances_are_written_only_for_ids_the_realm_knows_and_the_result_is_asserted() {
    let mut model = geared_level_eighty();
    model.wardrobe.active.insert(1, 100);
    model.wardrobe.active.insert(2, 200);
    model.wardrobe.outfits.insert("Sunday".into(), vec![100, 0]);
    model.wardrobe.outfits.insert("Far".into(), vec![100, 200]);
    model.wardrobe.can_see_spell = false;
    let k = super::knowledge::RealmKnowledge::new(crate::portable::collection::IdSet::from_ids([100]).unwrap(), Default::default());
    let u = users();
    let plan = build_plan(&model, &PlanContext { knowledge: Some(&k), ..ctx(&all_tables(), &u) }).unwrap();
    assert_eq!(plan.counts.wardrobe, 1 + 1 + 1, "one selection, one outfit, one visibility row");
    let script = &plan.script;
    assert!(script.contains("INSERT INTO acore_characters.`character_appearance` (`guid`, `category_id`, `appearance_id`) VALUES\n  (@char, 1, 100);"), "{script}");
    assert!(!script.contains("(@char, 2, 200)") && !script.contains(&hex::encode("100 200")), "ids the realm does not know are never written");
    assert!(script.contains("acore_characters.character_appearance WHERE guid = @char"), "the number of rows is asserted before the commit");
    let commit = script.find("COMMIT;").unwrap();
    assert!(script.find("character_appearance_outfit WHERE guid = @char").unwrap() < commit);

    let none = build_plan(&model, &PlanContext { knowledge: None, ..ctx(&all_tables(), &u) }).unwrap();
    assert_eq!(none.counts.wardrobe, 0);
    assert!(!none.script.contains("INSERT INTO acore_characters.`character_appearance`"));
    assert!(none.not_applied_settings.contains(&"wardrobe".to_string()), "reported as kept canonical, not applied");
}
