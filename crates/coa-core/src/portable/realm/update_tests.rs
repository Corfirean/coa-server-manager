//! Offline tests of the in-place update script: what it writes, what it must never write, and that hostile text stays inert.

use std::collections::HashMap;

use super::super::fixtures::{geared_level_eighty, id7};
use super::super::ids::{ContentId, PortableItemId, PortablePetId};
use super::super::model::*;
use super::script::SchemaProbe;
use super::update::*;

fn probe() -> SchemaProbe {
    let tables = ["reserved_name", "item_refund_instance", "item_soulbound_trade_data", "pet_aura", "pet_spell_cooldown", "character_pet_declinedname", "character_gifts"];
    SchemaProbe { tables: tables.iter().map(|t| t.to_string()).collect(), character_columns: Default::default() }
}

fn realm_ids(current: &PortableCharacter) -> (HashMap<PortableItemId, u32>, HashMap<PortablePetId, u32>) {
    (current.items.iter().enumerate().map(|(i, it)| (it.id, 5000 + i as u32)).collect(), current.pets.iter().enumerate().map(|(i, p)| (p.id, 70 + i as u32)).collect())
}

fn script_of(current: &PortableCharacter, merged: &PortableCharacter) -> UpdateScript {
    let (items, pets) = realm_ids(current);
    let users = ["acore".to_string()];
    let probe = probe();
    build_update(current, merged, &UpdateContext { ruleset: Ruleset::Coa, local_guid: 4242, revision: 9, nonce: [1, 2, 3, 4], game_server_users: &users, probe: &probe, items: &items, pets: &pets, session: None }).unwrap()
}

fn statements(script: &UpdateScript) -> Vec<&str> {
    script.script.lines().collect()
}

fn writes(script: &UpdateScript) -> Vec<&str> {
    statements(script).into_iter().filter(|l| l.starts_with("UPDATE ") || l.starts_with("INSERT ") || l.starts_with("DELETE ")).collect()
}

fn new_item(n: u64, slot: u8, container: Option<PortableItemId>) -> PortableItem {
    PortableItem {
        id: PortableItemId::from_uuid(id7(800_000 + n)).unwrap(),
        container,
        slot,
        entry: ContentId::new("coa", "item", 88_000 + n).unwrap(),
        count: 1,
        duration: 0,
        charges: vec![],
        flags: 0,
        enchantments: vec![],
        random_property_id: 0,
        durability: 40,
        played_time: 0,
        text: None,
        creator_name: None,
        gift: None,
    }
}

#[test]
fn an_unchanged_character_is_updated_by_nothing_but_the_marker() {
    let current = geared_level_eighty();
    let script = script_of(&current, &current.clone());
    assert!(script.counts.is_empty(), "{:?}", script.counts);
    let w = writes(&script);
    assert_eq!(w.len(), 2, "only the marker rows are written: {w:?}");
    assert!(w[0].starts_with("DELETE FROM acore_characters.`character_settings`"));
    assert!(w[1].starts_with("INSERT INTO acore_characters.`character_settings`"));
    assert!(script.script.contains("coa.portable.alloc") || script.script.contains(&hex::encode("coa.portable.alloc")));
}

#[test]
fn only_the_columns_that_changed_are_written_and_never_the_position() {
    let current = geared_level_eighty();
    let mut merged = current.clone();
    merged.progression.money += 12_345;
    merged.progression.level = 81;
    merged.progression.xp = 7;
    merged.progression.honor.total_kills += 3;
    let script = script_of(&current, &merged);
    let w = writes(&script);
    let updates: Vec<&&str> = w.iter().filter(|l| l.starts_with("UPDATE")).collect();
    assert_eq!(updates.len(), 1, "{w:?}");
    let u = updates[0];
    for column in ["`level`", "`xp`", "`money`", "`totalKills`"] {
        assert!(u.contains(column), "{u}");
    }
    assert!(!u.contains("`name`") && !u.contains("`totalHonorPoints`") && !u.contains("`skin`"), "unchanged columns are not written: {u}");
    assert!(u.ends_with("WHERE `guid` = @char;"));
    assert_eq!(script.counts.character_columns, 4);
    // nothing of the realm's own placement is ever mentioned
    for forbidden in ["character_homebind", "position_x", "position_y", "position_z", "`map`", "`zone`", "orientation", "logout_time", "`online`", "`account`", "`guid` = 4242"] {
        assert!(!script.script.contains(forbidden), "the update must not touch {forbidden}");
    }
    assert!(!script.script.contains("INSERT INTO acore_characters.`characters`") && !script.script.contains("DELETE FROM acore_characters.`characters`"), "the character row is never created or deleted");
    assert!(script.script.contains("SET @char := 4242;"), "the character is addressed by its existing local guid");
}

#[test]
fn removed_items_are_deleted_by_their_mapped_guid_and_new_ones_are_allocated_in_the_transaction() {
    let current = geared_level_eighty();
    let (ids, _) = realm_ids(&current);
    let mut merged = current.clone();
    let gone = merged.items.iter().position(|i| i.container.is_none() && i.slot >= 23 && !current.items.iter().any(|o| o.container == Some(i.id))).unwrap();
    let gone_id = merged.items.remove(gone).id;
    let fresh = new_item(1, 66, None);
    merged.items.push(fresh.clone());
    let merged = merged.normalized();
    let script = script_of(&current, &merged);
    assert_eq!(script.removed_items, vec![gone_id]);
    assert_eq!(script.added_items, vec![fresh.id]);
    let guid = ids[&gone_id];
    let all = script.script.clone();
    assert!(all.contains(&format!("DELETE FROM acore_characters.`item_instance` WHERE `owner_guid` = @char AND `guid` IN ({guid});")), "{all}");
    assert!(all.contains(&format!("DELETE FROM acore_characters.`character_inventory` WHERE `guid` = @char AND `item` IN ({guid});")));
    assert!(all.contains("(@item_base + 0)"), "the new item gets a guid allocated inside the transaction");
    assert_eq!((script.counts.items_added, script.counts.items_removed), (1, 1));
    // deletes run before updates before inserts
    let w = writes(&script);
    let first_insert = w.iter().position(|l| l.starts_with("INSERT")).unwrap();
    let last_delete = w.iter().rposition(|l| l.starts_with("DELETE")).unwrap();
    assert!(last_delete < first_insert || w[last_delete].contains("character_settings"), "{w:?}");
    // every assertion comes before the commit, and the commit is the only one
    let s = statements(&script);
    let commit = s.iter().position(|l| *l == "COMMIT;").unwrap();
    assert_eq!(s.iter().filter(|l| **l == "COMMIT;").count(), 1);
    assert!(s.iter().enumerate().filter(|(_, l)| l.starts_with("DO IF(") && l.contains("@items_before")).all(|(i, _)| i < commit));
}

#[test]
fn moved_items_free_their_places_before_anything_takes_them() {
    let current = geared_level_eighty();
    let mut merged = current.clone();
    // two backpack items swap places: a swap collides on the unique place key unless both rows leave first
    let free: Vec<usize> = merged.items.iter().enumerate().filter(|(_, i)| i.container.is_none() && (23..39).contains(&i.slot)).map(|(n, _)| n).take(2).collect();
    assert_eq!(free.len(), 2, "the fixture has two backpack items");
    let (a, b) = (merged.items[free[0]].slot, merged.items[free[1]].slot);
    merged.items[free[0]].slot = b;
    merged.items[free[1]].slot = a;
    let script = script_of(&current, &merged);
    assert_eq!(script.counts.items_moved, 2);
    let w = writes(&script);
    let delete = w.iter().position(|l| l.starts_with("DELETE FROM acore_characters.`character_inventory`")).unwrap();
    let insert = w.iter().position(|l| l.starts_with("INSERT INTO acore_characters.`character_inventory`")).unwrap();
    assert!(delete < insert, "{w:?}");
    assert!(w[delete].matches(',').count() >= 1, "both rows leave in one statement: {}", w[delete]);
}

#[test]
fn hostile_text_reaches_the_script_only_as_hex() {
    let current = geared_level_eighty();
    let mut merged = current.clone();
    let evil = "x'); DROP TABLE characters; -- \\ \" \n \0 /* */";
    merged.identity.name = "Renamed".into();
    merged.items.push({
        let mut i = new_item(2, 66, None);
        i.text = Some(evil.into());
        i
    });
    merged.settings.insert("core.ascension_build.77".into(), vec![1, 2, 3]);
    let script = script_of(&current, &merged.normalized());
    assert!(!script.script.contains("DROP TABLE"), "{}", script.script);
    assert!(!script.script.contains('\0'));
    assert!(script.script.contains(&hex::encode(evil)), "the text is there, as hex");
    assert!(script.script.contains(&hex::encode("Renamed")));
    // a rename is guarded against a taken or reserved name
    assert!(script.script.contains("reserved_name"));
    assert!(script.script.contains("guid <> @char"));
}

#[test]
fn quarantined_settings_are_never_written_even_if_a_model_carries_them() {
    let current = geared_level_eighty();
    let mut merged = current.clone();
    merged.settings.insert("core.spell_charge.804197".into(), vec![9]);
    merged.settings.insert("core.wildcard.cards".into(), vec![9]);
    merged.settings.insert("coa.portable.import".into(), vec![9]);
    let script = script_of(&current, &merged);
    for source in ["core.spell_charge.804197", "core.wildcard.cards", "coa.portable.import"] {
        assert!(!script.script.contains(&hex::encode(source)) || source == "coa.portable.import", "{source} must not be written");
    }
    // the marker the script writes is its own, not one smuggled in through the model
    assert_eq!(script.script.matches(&hex::encode("coa.portable.import")).count(), 2, "delete + insert of the importer's own marker only");
}

#[test]
fn what_a_character_is_cannot_be_changed_in_place() {
    let current = geared_level_eighty();
    let (items, pets) = realm_ids(&current);
    let users = ["acore".to_string()];
    let probe = probe();
    let ctx = UpdateContext { ruleset: Ruleset::Coa, local_guid: 1, revision: 2, nonce: [0; 4], game_server_users: &users, probe: &probe, items: &items, pets: &pets, session: None };
    let mut merged = current.clone();
    merged.identity.gender ^= 1;
    assert!(build_update(&current, &merged, &ctx).is_err(), "gender");
    let mut merged = current.clone();
    merged.identity.class = ContentId::new("coa", "class", 99).unwrap();
    assert!(build_update(&current, &merged, &ctx).is_err(), "class");
    let mut merged = current.clone();
    merged.ruleset = Ruleset::Wildcard;
    assert!(build_update(&current, &merged, &ctx).is_err(), "ruleset");
    let mut merged = current.clone();
    if let Some(p) = merged.pets.first_mut() {
        p.pet_type ^= 1;
        assert!(build_update(&current, &merged, &ctx).is_err(), "pet kind");
    }
}

#[test]
fn a_new_pet_is_allocated_in_the_transaction_and_a_removed_pet_leaves_every_table() {
    let current = geared_level_eighty();
    assert!(!current.pets.is_empty(), "the fixture has a pet");
    let removed = current.pets[0].id;
    let mut merged = current.clone();
    let mut pet = merged.pets.remove(0);
    pet.id = PortablePetId::from_uuid(id7(777_000)).unwrap();
    merged.pets.push(pet);
    let script = script_of(&current, &merged);
    assert_eq!((script.removed_pets.clone(), script.added_pets.len()), (vec![removed], 1));
    for table in ["character_pet", "pet_spell", "pet_aura", "pet_spell_cooldown", "character_pet_declinedname"] {
        assert!(script.script.contains(&format!("DELETE FROM acore_characters.`{table}` WHERE")), "{table}");
    }
    assert!(script.script.contains("(@pet_base + 0)"));
}

#[test]
fn the_report_parses_and_a_lost_commit_is_visible() {
    assert_eq!(parse_update_report("#R:update\t4242\t900\t12\n#R:committed\n").unwrap(), ((4242, 900, 12), true));
    assert_eq!(parse_update_report("#R:update\t4242\t900\t12\n").unwrap(), ((4242, 900, 12), false));
    assert!(parse_update_report("").is_err());
    assert!(parse_update_report("#R:alloc\t1\t2\t3\t4\t5\n").is_err());
}
