//! The complete A -> B -> play -> reconcile -> A -> B round trip against **real worldservers**, in stages.
//!
//! Each stage is an `#[ignore]`d test that runs with the realms' worldservers STOPPED; between stages a real worldserver of
//! the realm that is being worked on loads every character (`botcmd spawnbot <guid>` runs the real `Player::LoadFromDB`),
//! saves them (`saveall`) and shuts down. The driver is `tools/live_roundtrip.ps1`; this file holds the stages:
//!
//! ```text
//!   rt1_join              A fixture characters -> portable -> imported into B
//!   [ server B ]          real load + save of the arrivals                                  = the realm's own normalisation (B0)
//!   rt2_baseline_and_play capture B0, then play on B (SQL, valid real ids)
//!   [ server B ]          real load + save + a real `character level`                       = B1
//!   rt3_reconcile         reconcile B1 into canonical, update the ORIGINAL characters of A in place
//!   [ server A ]          real load + save of the updated originals
//!   rt4_check_a           A still loads and holds what it was given; play on A; reconcile; update B in place
//!   [ server B ]          real load + save of the updated characters
//!   rt5_check_b           final report
//! ```
//!
//! `COA_PORTABLE_RT_DIR` names the directory with the Manager store and the state files of the run.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::portable::ids::{CharacterId, PortableItemId};
use crate::portable::model::*;
use crate::portable::store::Store;

use super::live_import::{
    expected_after_a_trip, first, json_diff, make, number, opts, realms, reset_b, Realms, ACCOUNT,
    B,
};
use super::*;

const A: &str = "realm-a";
/// What the original characters of realm A are called in the store: the make_portable() binding.
const CHARACTERS: [u32; 3] = [1002, 1004, 1005];

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Joined {
    a_guid: u32,
    id: String,
    b_guid: u32,
}

fn dir() -> Option<PathBuf> {
    std::env::var("COA_PORTABLE_RT_DIR").ok().map(PathBuf::from)
}

fn joined(dir: &Path) -> Vec<Joined> {
    serde_json::from_slice(
        &std::fs::read(dir.join("joined.json")).expect("joined.json: run rt1 first"),
    )
    .unwrap()
}

fn id_of(j: &Joined) -> CharacterId {
    j.id.parse().unwrap()
}

fn sql(db: &Db, text: &str) {
    db.query(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
}

fn spells_of(db: &Db, guid: u32) -> Vec<u32> {
    db.query(&format!(
        "SELECT spell FROM acore_characters.character_spell WHERE guid = {guid} ORDER BY spell"
    ))
    .unwrap()
    .lines()
    .filter_map(|l| l.trim().parse().ok())
    .collect()
}

fn world_local(db: &Db, guid: u32) -> Vec<String> {
    let one = |sql: String| db.query(&sql).unwrap().trim().to_string();
    vec![
        one(format!("SELECT CONCAT_WS('|', guid, account, map, zone, position_x, position_y, position_z, orientation, instance_id) FROM acore_characters.characters WHERE guid = {guid}")),
        one(format!("SELECT CONCAT_WS('|', mapId, zoneId, posX, posY, posZ) FROM acore_characters.character_homebind WHERE guid = {guid}")),
        one(format!("SELECT GROUP_CONCAT(CONCAT_WS('|', spell, item, time) ORDER BY spell) FROM acore_characters.character_spell_cooldown WHERE guid = {guid}")),
        one(format!("SELECT GROUP_CONCAT(CONCAT_WS('|', quest, time) ORDER BY quest) FROM acore_characters.character_queststatus_daily WHERE guid = {guid}")),
        one(format!("SELECT GROUP_CONCAT(CONCAT_WS('|', instance, permanent) ORDER BY instance) FROM acore_characters.character_instance WHERE guid = {guid}")),
    ]
}

fn free_slot(db: &Db, guid: u32) -> u8 {
    let used: Vec<u64> = db
        .query(&format!(
            "SELECT slot FROM acore_characters.character_inventory WHERE guid = {guid} AND bag = 0"
        ))
        .unwrap()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    (23u8..=38)
        .chain(39..=66)
        .find(|s| !used.contains(&(*s as u64)))
        .expect("a free slot")
}

fn realm_gives(db: &Db, guid: u32, entry: u64) -> u32 {
    let item = number(
        db,
        "SELECT MAX(guid) + 1 FROM acore_characters.item_instance",
    ) as u32;
    let slot = free_slot(db, guid);
    let enchants = "0 ".repeat(36);
    sql(
        db,
        &format!(
            "INSERT INTO acore_characters.item_instance (guid, itemEntry, owner_guid, count, enchantments) VALUES ({item}, {entry}, {guid}, 1, '{enchants}');
             INSERT INTO acore_characters.character_inventory (guid, bag, slot, item) VALUES ({guid}, 0, {slot}, {item});"
        ),
    );
    item
}

fn plain(m: &PortableCharacter) -> Vec<PortableItemId> {
    let containers: std::collections::HashSet<PortableItemId> =
        m.items.iter().filter_map(|i| i.container).collect();
    m.items
        .iter()
        .filter(|i| i.container.is_none() && !containers.contains(&i.id) && i.slot >= 23)
        .map(|i| i.id)
        .collect()
}

fn local_item(store: &Store, id: CharacterId, server: &str, item: PortableItemId) -> Option<u32> {
    store
        .item_mappings(id, server)
        .unwrap()
        .into_iter()
        .find(|m| m.portable_item_id == item && m.active)
        .map(|m| m.local_item_guid)
}

fn read_server(r: &Realms, store: &Store, id: CharacterId, server: &str) -> Exported {
    let db = if server == B { &r.b } else { &r.a };
    let guid = store
        .server_mappings(id)
        .unwrap()
        .into_iter()
        .find(|m| m.server_id == server)
        .unwrap()
        .local_guid;
    export_character_with_pets(
        db,
        guid,
        Some(id),
        &store.active_item_lookup(id, server).unwrap(),
        &store.active_pet_lookup(id, server).unwrap(),
    )
    .unwrap_or_else(|e| panic!("export of {guid} from {server}: {e}"))
}

fn local_guid(store: &Store, id: CharacterId, server: &str) -> u32 {
    store
        .server_mappings(id)
        .unwrap()
        .into_iter()
        .find(|m| m.server_id == server)
        .unwrap()
        .local_guid
}

fn say(dir: &Path, line: impl AsRef<str>) {
    use std::io::Write;
    println!("{}", line.as_ref());
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("report.txt"))
        .unwrap();
    writeln!(f, "{}", line.as_ref()).unwrap();
}

#[test]
#[ignore]
fn rt1_join() {
    let (Some(r), Some(dir)) = (realms(), dir()) else {
        return;
    };
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    reset_b(&r.b);
    let mut store = Store::open(&dir).unwrap();
    let profile = store.default_profile().unwrap();
    let mut out = Vec::new();
    for a_guid in CHARACTERS {
        let id = make(&r, &mut store, profile, a_guid);
        let o = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts())
            .unwrap_or_else(|e| panic!("import of {a_guid}: {e}"));
        say(
            &dir,
            format!(
                "rt1: A {a_guid} -> portable {id} -> B {} (\"{}\", {} items, {} pets)",
                o.local_guid, o.final_name, o.items, o.pets
            ),
        );
        out.push(Joined {
            a_guid,
            id: id.to_string(),
            b_guid: o.local_guid,
        });
    }
    std::fs::write(dir.join("joined.json"), serde_json::to_vec(&out).unwrap()).unwrap();
}

#[test]
#[ignore]
fn rt2_baseline_and_play() {
    let (Some(r), Some(dir)) = (realms(), dir()) else {
        return;
    };
    let mut store = Store::open(&dir).unwrap();
    let joined = joined(&dir);
    for j in &joined {
        let id = id_of(j);
        let start = begin_session(&r.b, &mut store, id, B)
            .unwrap_or_else(|e| panic!("baseline of {}: {e}", j.b_guid));
        say(
            &dir,
            format!(
                "rt2: B0 of {} captured: {} item(s) held back by the realm, {} of its own, {} pet(s) held back, {} of its own; warnings: {}",
                j.b_guid,
                start.items_filtered,
                start.items_realm_local,
                start.pets_filtered,
                start.pets_realm_local,
                start.warnings.len()
            ),
        );
        let c0 = store.synced_model(id, B).unwrap().unwrap();
        let b0 = store.open_baseline(id, B).unwrap().unwrap().b0;
        // what the real server did to the arrival, as the three-way model sees it
        let mut diffs = Vec::new();
        json_diff(
            "",
            &serde_json::to_value(&c0).unwrap(),
            &serde_json::to_value(&b0).unwrap(),
            &mut diffs,
        );
        say(
            &dir,
            format!(
                "rt2:   the realm's own normalisation of {}: {} difference(s) between C0 and B0{}",
                j.b_guid,
                diffs.len(),
                if diffs.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n     {}",
                        diffs
                            .iter()
                            .take(40)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n     ")
                    )
                }
            ),
        );
    }

    // ---- play on B: valid real ids only -----------------------------------------------------------------------------------------
    let g = |i: usize| joined[i].b_guid;
    // 1002: money, a sold item, loot, a spell the character did not have, a reputation point
    {
        let id = id_of(&joined[0]);
        let c0 = store.synced_model(id, B).unwrap().unwrap();
        let candidates: Vec<PortableItemId> = plain(&c0)
            .into_iter()
            .filter(|i| local_item(&store, id, B, *i).is_some())
            .collect();
        let sold = local_item(&store, id, B, candidates[2]).unwrap();
        sql(&r.b, &format!("DELETE FROM acore_characters.character_inventory WHERE guid = {0} AND item = {1}; DELETE FROM acore_characters.item_instance WHERE guid = {1} AND owner_guid = {0};", g(0), sold));
        let loot_entry = c0
            .items
            .iter()
            .find(|i| i.id == candidates[4])
            .unwrap()
            .entry
            .id();
        realm_gives(&r.b, g(0), loot_entry);
        let extra = spells_of(&r.a, 1004)
            .into_iter()
            .find(|s| !spells_of(&r.b, g(0)).contains(s))
            .unwrap();
        sql(&r.b, &format!("UPDATE acore_characters.characters SET money = money + 777000, totalKills = totalKills + 12 WHERE guid = {0}; INSERT INTO acore_characters.character_spell (guid, spell, specMask) VALUES ({0}, {1}, 255);", g(0), extra));
        sql(&r.b, &format!("UPDATE acore_characters.character_reputation SET standing = standing + 100 WHERE guid = {} LIMIT 1", g(0)));
        say(&dir, format!("rt2: played on B {}: +777000 money, +12 kills, sold item {sold}, looted entry {loot_entry}, learned spell {extra}, +100 reputation", g(0)));
    }
    // 1004: a carried build/spec setting changes and honor accumulates
    {
        let row = first(&r.b, &format!("SELECT CONCAT(source, '|', data) FROM acore_characters.character_settings WHERE guid = {} AND source LIKE 'core.ascension_build.%' LIMIT 1", g(1)));
        let (source, data) = row.split_once('|').expect("a carried build setting");
        let mut words: Vec<u64> = data
            .split_whitespace()
            .map(|w| w.parse().unwrap())
            .collect();
        *words.last_mut().unwrap() += 1;
        let text: String = words.iter().map(|w| format!("{w} ")).collect();
        sql(&r.b, &format!("UPDATE acore_characters.character_settings SET data = '{text}' WHERE guid = {} AND source = '{source}'", g(1)));
        sql(&r.b, &format!("UPDATE acore_characters.characters SET totalHonorPoints = totalHonorPoints + 321, money = money + 5 WHERE guid = {}", g(1)));
        say(
            &dir,
            format!(
                "rt2: played on B {}: setting {source} changed, +321 honor, +5 money",
                g(1)
            ),
        );
    }
    // 1005: pets
    {
        let numbers: Vec<u32> = store
            .pet_mappings(id_of(&joined[2]), B)
            .unwrap()
            .iter()
            .map(|m| m.local_pet_number)
            .collect();
        sql(&r.b, &format!("UPDATE acore_characters.character_pet SET level = level + 1, exp = 4321, name = 'Played' WHERE id = {}", numbers[0]));
        sql(&r.b, &format!("DELETE FROM acore_characters.character_pet WHERE id = {0}; DELETE FROM acore_characters.pet_spell WHERE guid = {0};", numbers[1]));
        sql(
            &r.b,
            &format!(
                "UPDATE acore_characters.characters SET money = money + 99 WHERE guid = {}",
                g(2)
            ),
        );
        say(
            &dir,
            format!(
                "rt2: played on B {}: pet {} levelled and renamed, pet {} abandoned, +99 money",
                g(2),
                numbers[0],
                numbers[1]
            ),
        );
    }
}

#[test]
#[ignore]
fn rt3_reconcile_and_update_a() {
    let (Some(r), Some(dir)) = (realms(), dir()) else {
        return;
    };
    let mut store = Store::open(&dir).unwrap();
    let joined = joined(&dir);
    sql(&r.a, "UPDATE acore_characters.characters SET online = 0");
    for (i, j) in joined.iter().enumerate() {
        let id = id_of(j);
        let c0 = store.synced_model(id, B).unwrap().unwrap();
        let b0 = store.open_baseline(id, B).unwrap().unwrap().b0;
        let b1 = read_server(&r, &store, id, B).model;

        let out = reconcile_session(&r.b, &mut store, id, B, false, Some("rt3 checkpoint"))
            .unwrap_or_else(|e| panic!("reconcile of {}: {e}", j.b_guid));
        let canonical = store.load_current(id).unwrap();
        say(&dir, format!("rt3: reconciled {} -> canonical revision {} ({} change(s) applied, {} left alone)\n     {}", j.b_guid, out.revision, out.changes.len(), out.left_alone.len(), out.changes.iter().take(30).cloned().collect::<Vec<_>>().join("\n     ")));

        // independent expectations from the three snapshots (not from the merge engine)
        let level_expected = if b1.progression.level != b0.progression.level {
            b1.progression.level
        } else {
            c0.progression.level
        };
        assert_eq!(
            canonical.progression.level, level_expected,
            "{} level",
            j.b_guid
        );
        if b1.progression.level == b0.progression.level {
            let money = (c0.progression.money as i64 + b1.progression.money as i64
                - b0.progression.money as i64)
                .clamp(0, u32::MAX as i64) as u32;
            assert_eq!(
                canonical.progression.money, money,
                "{} money: C0 + (B1 - B0), once",
                j.b_guid
            );
        }
        // items the realm showed at B0 have stable portable ids; items first seen in B1 get their ids when they are exported, so
        // those are compared by entry
        let ids = |m: &PortableCharacter| -> std::collections::BTreeSet<PortableItemId> {
            m.items.iter().map(|i| i.id).collect()
        };
        let (c, b0i, b1i) = (ids(&c0), ids(&b0), ids(&b1));
        let lost: std::collections::BTreeSet<_> = b0i
            .difference(&b1i)
            .filter(|i| c.contains(i))
            .copied()
            .collect();
        let canonical_ids = ids(&canonical);
        let carried: std::collections::BTreeSet<_> = canonical_ids
            .iter()
            .filter(|i| c.contains(i))
            .copied()
            .collect();
        assert_eq!(
            carried,
            c.difference(&lost).copied().collect(),
            "{}: C0 items = kept, minus what the player lost; what the realm filtered at B0 stays",
            j.b_guid
        );
        let entries = |m: &PortableCharacter, keep: &dyn Fn(&PortableItemId) -> bool| -> Vec<u64> {
            let mut v: Vec<u64> = m
                .items
                .iter()
                .filter(|i| keep(&i.id))
                .map(|i| i.entry.id())
                .collect();
            v.sort_unstable();
            v
        };
        assert_eq!(
            entries(&canonical, &|i| !c.contains(i)),
            entries(&b1, &|i| !b0i.contains(i)),
            "{}: what is new in the canonical character is exactly what the player gained",
            j.b_guid
        );
        let spell_set = |m: &PortableCharacter| -> std::collections::BTreeSet<u32> {
            m.build.spells.iter().map(|(s, _)| *s).collect()
        };
        let (cs, b0s, b1s) = (spell_set(&c0), spell_set(&b0), spell_set(&b1));
        let expected_spells: std::collections::BTreeSet<u32> = cs
            .iter()
            .chain(b1s.difference(&b0s))
            .copied()
            .filter(|s| !(b0s.contains(s) && !b1s.contains(s)))
            .collect();
        assert_eq!(spell_set(&canonical), expected_spells, "{}: spells = C0 + learned - unlearned; the realm's own default spells are not progress", j.b_guid);
        // a repeated checkpoint does not count anything twice
        let again = reconcile_session(&r.b, &mut store, id, B, false, None).unwrap();
        assert_eq!(
            again.revision, out.revision,
            "{}: the same B1 makes no new revision",
            j.b_guid
        );
        // close the session
        let closed = reconcile_session(&r.b, &mut store, id, B, true, None).unwrap();
        assert_eq!(closed.revision, out.revision);
        assert!(store.open_baseline(id, B).unwrap().is_none());

        // ---- update the ORIGINAL character of realm A in place ----------------------------------------------------------------
        let a_before = world_local(&r.a, j.a_guid);
        let chars_before = number(&r.a, "SELECT COUNT(*) FROM acore_characters.characters");
        let upd = update_realm_character(&r.a, &mut store, id, A, &opts())
            .unwrap_or_else(|e| panic!("update of A {}: {e}", j.a_guid));
        assert!(upd.updated, "{}", j.a_guid);
        say(
            &dir,
            format!(
                "rt3: updated A {} in place to revision {}: {:?}",
                j.a_guid, upd.to_revision, upd.counts
            ),
        );
        assert_eq!(
            world_local(&r.a, j.a_guid),
            a_before,
            "A {}: position, homebind and local data are untouched",
            j.a_guid
        );
        assert_eq!(
            number(&r.a, "SELECT COUNT(*) FROM acore_characters.characters"),
            chars_before
        );
        assert_eq!(local_guid(&store, id, A), j.a_guid);
        let back = read_server(&r, &store, id, A).model;
        let mut diffs = Vec::new();
        json_diff(
            "",
            &serde_json::to_value(&canonical).unwrap(),
            &serde_json::to_value(&back).unwrap(),
            &mut diffs,
        );
        assert!(
            diffs.is_empty(),
            "A {} holds the canonical character: {diffs:#?}",
            j.a_guid
        );
        let _ = i;
    }
    say(&dir, "rt3: done");
}

#[test]
#[ignore]
fn rt4_check_a_then_play_on_a_and_update_b() {
    let (Some(r), Some(dir)) = (realms(), dir()) else {
        return;
    };
    let mut store = Store::open(&dir).unwrap();
    let joined = joined(&dir);
    for j in &joined {
        let id = id_of(j);
        let canonical = store.load_current(id).unwrap();
        let online = number(
            &r.a,
            &format!(
                "SELECT online FROM acore_characters.characters WHERE guid = {}",
                j.a_guid
            ),
        );
        let back = export_character_with_pets(
            &r.a,
            j.a_guid,
            Some(id),
            &store.active_item_lookup(id, A).unwrap(),
            &store.active_pet_lookup(id, A).unwrap(),
        );
        match back {
            Err(e) => say(&dir, format!("rt4: A {} (online {online}) cannot be exported after the real server's save: {e}", j.a_guid)),
            Ok(e) => {
                let mut diffs = Vec::new();
                json_diff("", &serde_json::to_value(&canonical).unwrap(), &serde_json::to_value(&e.model).unwrap(), &mut diffs);
                say(&dir, format!("rt4: A {} after a REAL load + save: {} difference(s) from canonical revision {}{}", j.a_guid, diffs.len(), store.character(id).unwrap().revision, if diffs.is_empty() { String::new() } else { format!("\n     {}", diffs.iter().take(40).cloned().collect::<Vec<_>>().join("\n     ")) }));
                assert_eq!(e.model.progression.money, canonical.progression.money, "A {} money survived the real load", j.a_guid);
                assert_eq!(e.model.progression.level, canonical.progression.level);
            }
        }
    }
    // play on A: a session of its own on the original characters
    for j in &joined {
        let id = id_of(j);
        let s = begin_session(&r.a, &mut store, id, A)
            .unwrap_or_else(|e| panic!("baseline on A {}: {e}", j.a_guid));
        say(
            &dir,
            format!(
                "rt4: baseline on A {}: {} filtered, {} realm-local item(s)",
                j.a_guid, s.items_filtered, s.items_realm_local
            ),
        );
        sql(
            &r.a,
            &format!(
                "UPDATE acore_characters.characters SET money = money + 1000 WHERE guid = {}",
                j.a_guid
            ),
        );
    }
    for j in &joined {
        let id = id_of(j);
        let before = store.load_current(id).unwrap();
        let out =
            reconcile_session(&r.a, &mut store, id, A, true, Some("rt4 session on A")).unwrap();
        let after = store.load_current(id).unwrap();
        assert_eq!(
            after.progression.money,
            before.progression.money + 1000,
            "A {}: +1000 once",
            j.a_guid
        );
        say(
            &dir,
            format!(
                "rt4: reconciled A {} -> canonical revision {}",
                j.a_guid, out.revision
            ),
        );
    }
    // back to B: the SAME local characters, updated in place
    sql(&r.b, "UPDATE acore_characters.characters SET online = 0");
    for j in &joined {
        let id = id_of(j);
        let local_before = world_local(&r.b, j.b_guid);
        let chars_before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters");
        let held = number(
            &r.b,
            &format!(
                "SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = {}",
                j.b_guid
            ),
        );
        let upd = update_realm_character(&r.b, &mut store, id, B, &opts())
            .unwrap_or_else(|e| panic!("update of B {}: {e}", j.b_guid));
        assert!(upd.updated);
        say(
            &dir,
            format!(
                "rt4: updated B {} IN PLACE (revision {} -> {}): {:?}",
                j.b_guid, upd.from_revision, upd.to_revision, upd.counts
            ),
        );
        assert_eq!(
            world_local(&r.b, j.b_guid),
            local_before,
            "B {}: the realm's own placement survived the update",
            j.b_guid
        );
        assert_eq!(
            number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"),
            chars_before,
            "never deleted or re-created"
        );
        assert_eq!(local_guid(&store, id, B), j.b_guid);
        let _ = held;
        let canonical = store.load_current(id).unwrap();
        let back = read_server(&r, &store, id, B).model;
        assert_eq!(
            back.progression.money, canonical.progression.money,
            "B {} money",
            j.b_guid
        );
    }
}

#[test]
#[ignore]
fn rt5_check_b() {
    let (Some(r), Some(dir)) = (realms(), dir()) else {
        return;
    };
    let store = Store::open(&dir).unwrap();
    for j in &joined(&dir) {
        let id = id_of(j);
        let canonical = store.load_current(id).unwrap();
        let e = read_server(&r, &store, id, B);
        let mut diffs = Vec::new();
        json_diff(
            "",
            &serde_json::to_value(expected_after_a_trip(&canonical, &e.model)).unwrap(),
            &serde_json::to_value(&e.model).unwrap(),
            &mut diffs,
        );
        say(&dir, format!("rt5: B {} after the update and a REAL load + save: {} difference(s) from canonical revision {}{}", j.b_guid, diffs.len(), store.character(id).unwrap().revision, if diffs.is_empty() { String::new() } else { format!("\n     {}", diffs.iter().take(40).cloned().collect::<Vec<_>>().join("\n     ")) }));
        assert_eq!(e.model.progression.money, canonical.progression.money);
        assert!(
            store.open_imports(B).unwrap().is_empty() && store.open_imports(A).unwrap().is_empty()
        );
    }
    say(&dir, "rt5: done");
}
