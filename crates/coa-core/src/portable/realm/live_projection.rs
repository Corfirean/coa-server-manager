//! Phase 8 against the two real disposable MySQL servers of `live_import.rs`, no worldserver: a level-80 character is imported into a realm
//! whose cap is 60 (the decision of the core is supplied: the real core is exercised by `session::live_projection`), played there offline,
//! reconciled, updated for another cap and brought back to a realm that holds all of it.
//!
//! ```text
//! COA_PORTABLE_LIVE="<bin>|13998|<pw>"  COA_PORTABLE_LIVE_B="<bin>|13997|<pw>"  cargo test -p coa-core portable::realm::live_projection -- --ignored --test-threads=1
//! ```

use crate::db::Db;
use crate::portable::capabilities::RealmCapabilities;
use crate::portable::ids::{CharacterId, ContentId, PortableItemId, ProfileId};
use crate::portable::model::*;
use crate::portable::projection::{
    subject_of, Oracle, ProjectionHold, SuppliedDecision, POLICY_VERSION, PROTOCOL,
};
use crate::portable::store::Store;

use super::live_import::{fresh_store, make, number, opts, realms, reset_b, Realms, ACCOUNT, B};
use super::project::testing::caps;
use super::*;

/// Items of the custom content that keep their authored level 80 on a cap-60 realm.
pub(crate) const HELMET: u64 = 2_061_035;
pub(crate) const LEGS: u64 = 2_069_385;
/// Abilities of a ranger that need more than level 60 (every table that grants them says so).
pub(crate) const HIGH_SPELLS: [u32; 3] = [501_722, 501_723, 501_733];

pub(crate) fn item(like: &PortableCharacter, entry: u64, slot: u8) -> PortableItem {
    let mut i = like
        .items
        .iter()
        .find(|i| i.container.is_none())
        .expect("an item to copy")
        .clone();
    i.id = PortableItemId::new();
    i.container = None;
    i.slot = slot;
    i.entry = ContentId::new("coa", "item", entry).unwrap();
    i.count = 1;
    i.enchantments.clear();
    i.gift = None;
    i.text = None;
    i
}

/// The fixture's level-80 ranger with a helmet that only a level-80 can wear (worn), legwraps of the same kind in the backpack, and
/// abilities that exist only above level 60. Returns the id of the canonical character.
pub(crate) fn fixture_character(r: &Realms, owner: &mut Store, profile: ProfileId) -> CharacterId {
    let id = make(r, owner, profile, 1002);
    let mut m = owner.load_current(id).unwrap();
    m.items
        .retain(|i| !(i.container.is_none() && (i.slot == 0 || i.slot == 25)));
    let (helmet, legs) = (item(&m, HELMET, 0), item(&m, LEGS, 25));
    m.items.extend([helmet, legs]);
    m.build.spells.extend(HIGH_SPELLS.map(|s| (s, 255)));
    let revision = owner.character(id).unwrap().revision;
    owner
        .commit_snapshot(
            id,
            revision,
            m.normalized(),
            "realm-a",
            Some("test content above level 60"),
        )
        .unwrap();
    id
}

/// What a core at `cap` would hold of this character: the helmet, and the abilities of the list.
fn hold_at(c: &PortableCharacter, cap: u32, signature: &str, spells: &[u32]) -> ProjectionHold {
    let helmet = c
        .items
        .iter()
        .find(|i| i.entry.id() == HELMET)
        .map(|i| i.id);
    ProjectionHold {
        protocol: PROTOCOL,
        policy_version: POLICY_VERSION,
        progression_signature: signature.into(),
        max_player_level: cap,
        canonical_level: c.progression.level as u32,
        projected_level: cap,
        held_items: if cap < 80 {
            helmet.into_iter().collect()
        } else {
            vec![]
        },
        held_spells: spells.to_vec(),
        held_actions: vec![],
        settings: vec![],
        blocked_settings: vec![],
        subject: subject_of(c).unwrap(),
    }
}

fn options(caps: RealmCapabilities, decision: Option<ProjectionHold>) -> ImportOptions {
    let mut o = opts();
    o.capabilities = Some(std::sync::Arc::new(caps));
    o.projection = decision.map(|h| Oracle(std::sync::Arc::new(SuppliedDecision(h))));
    o
}

fn sql(db: &Db, text: &str) {
    db.query(text).unwrap_or_else(|e| panic!("{e}\n{text}"));
}

fn spells(db: &Db, guid: u32) -> std::collections::BTreeSet<u32> {
    db.query(&format!(
        "SELECT spell FROM acore_characters.character_spell WHERE guid = {guid}"
    ))
    .unwrap()
    .lines()
    .filter_map(|l| l.trim().parse().ok())
    .collect()
}

fn has_item(db: &Db, guid: u32, entry: u64) -> bool {
    number(db, &format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = {guid} AND itemEntry = {entry}")) > 0
}

fn level(db: &Db, guid: u32) -> u64 {
    number(
        db,
        &format!("SELECT level FROM acore_characters.characters WHERE guid = {guid}"),
    )
}

fn pin(db: &Db, guid: u32) -> String {
    db.query(&format!("SELECT data FROM acore_characters.character_settings WHERE guid = {guid} AND source = 'coa.portable.pin'")).unwrap().trim().to_string()
}

fn words(c: &RealmCapabilities) -> String {
    c.progression
        .as_ref()
        .unwrap()
        .pin_words()
        .iter()
        .map(|w| w.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

const SIG60: &str = "11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa11aa";
const SIG70: &str = "22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb22bb";
const SIG80: &str = "33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc33cc";

#[test]
#[ignore]
fn a_projected_character_is_played_reconciled_and_brought_back_to_a_realm_that_holds_all_of_it() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = fixture_character(&r, &mut store, profile);
    let c0 = store.load_current(id).unwrap();
    assert_eq!(c0.progression.level, 80);

    // ---- import: the realm's cap is 60 ----
    let caps60 = caps(60, SIG60);
    let o60 = options(caps60.clone(), Some(hold_at(&c0, 60, SIG60, &HIGH_SPELLS)));
    let outcome = import_character(&r.b, &mut store, id, B, ACCOUNT, &o60).unwrap();
    let guid = outcome.local_guid;
    assert_eq!(
        (
            level(&r.b, guid),
            number(
                &r.b,
                &format!("SELECT xp FROM acore_characters.characters WHERE guid = {guid}")
            )
        ),
        (60, 0)
    );
    assert!(
        !has_item(&r.b, guid, HELMET),
        "the helmet that only a level-80 can wear never reached the realm"
    );
    assert!(has_item(&r.b, guid, LEGS), "the same kind of item in the backpack did: the realm loads it and refuses to wear it by itself");
    assert!(HIGH_SPELLS.iter().all(|s| !spells(&r.b, guid).contains(s)));
    assert_eq!(pin(&r.b, guid), words(&caps60));
    let ctx = store.projection_context(id, B).unwrap().expect("projected");
    assert_eq!(
        (
            ctx.canonical_level,
            ctx.projected_level,
            ctx.progression_signature.as_str()
        ),
        (80, 60, SIG60)
    );
    assert_eq!(
        store
            .character_pin(id, B)
            .unwrap()
            .unwrap()
            .progression_signature,
        SIG60
    );
    assert_eq!(
        store.character(id).unwrap().revision,
        store
            .server_mappings(id)
            .unwrap()
            .into_iter()
            .find(|m| m.server_id == B)
            .unwrap()
            .last_revision
    );

    // a projected import without a decision is refused before anything is written
    let before = number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters");
    let other = make(&r, &mut store, profile, 1004);
    let mut m = store.load_current(other).unwrap();
    m.progression.level = 80;
    let revision = store.character(other).unwrap().revision;
    store
        .commit_snapshot(other, revision, m, "realm-a", None)
        .unwrap();
    let refused = import_character(
        &r.b,
        &mut store,
        other,
        B,
        ACCOUNT,
        &options(caps60.clone(), None),
    )
    .unwrap_err();
    assert!(
        matches!(
            refused,
            crate::portable::PortableError::ProjectionNeedsRunningCore {
                character_level: 80,
                cap: 60
            } | crate::portable::PortableError::Incompatible { .. }
        ),
        "{refused}"
    );
    assert_eq!(
        number(&r.b, "SELECT COUNT(*) FROM acore_characters.characters"),
        before,
        "nothing was written"
    );

    // ---- play offline: the realm's own level and xp move, gold, an item and reputation are earned ----
    begin_session(&r.b, &mut store, id, B).unwrap();
    sql(&r.b, &format!("UPDATE acore_characters.characters SET level = 59, xp = 1234, money = money + 7777 WHERE guid = {guid}"));
    let earned = number(
        &r.b,
        "SELECT MAX(guid) + 1 FROM acore_characters.item_instance",
    ) as u32;
    sql(&r.b, &format!(
        "INSERT INTO acore_characters.item_instance (guid, itemEntry, owner_guid, count, enchantments) VALUES ({earned}, 70001, {guid}, 1, '{}');
         INSERT INTO acore_characters.character_inventory (guid, bag, slot, item) VALUES ({guid}, 0, 60, {earned});",
        "0 ".repeat(36)
    ));
    let rewarded = reconcile_session(&r.b, &mut store, id, B, false, Some("offline play")).unwrap();
    assert!(rewarded.new_revision);
    let c1 = store.load_current(id).unwrap();
    assert_eq!(
        (c1.progression.level, c1.progression.xp),
        (80, c0.progression.xp),
        "the canonical level and xp are not the realm's"
    );
    assert_eq!(c1.progression.money, c0.progression.money + 7777);
    assert!(
        c1.items.iter().any(|i| i.entry.id() == 70001),
        "the gain arrived"
    );
    assert!(
        c1.items.iter().any(|i| i.entry.id() == HELMET),
        "the held helmet is canonical"
    );
    assert!(
        HIGH_SPELLS
            .iter()
            .all(|s| c1.build.spells.iter().any(|(k, _)| k == s)),
        "the held abilities are canonical"
    );
    // the same checkpoint again counts nothing twice
    let again = reconcile_session(&r.b, &mut store, id, B, false, Some("again")).unwrap();
    assert!(!again.new_revision, "{:?}", again.changes);
    reconcile_session(&r.b, &mut store, id, B, true, Some("close")).unwrap();
    assert_eq!(
        store
            .projection_context(id, B)
            .unwrap()
            .unwrap()
            .canonical_revision,
        store.character(id).unwrap().revision,
        "the context follows the revision"
    );

    // ---- back to a realm that holds all of it: the same realm, now with a cap of 80 (a native profile) ----
    let caps80 = caps(80, SIG80);
    let up =
        update_realm_character(&r.b, &mut store, id, B, &options(caps80.clone(), None)).unwrap();
    assert!(up.updated);
    assert_eq!(level(&r.b, guid), 80);
    assert!(has_item(&r.b, guid, HELMET), "the held helmet is restored");
    assert!(
        HIGH_SPELLS.iter().all(|s| spells(&r.b, guid).contains(s)),
        "the held abilities are restored"
    );
    assert_eq!(pin(&r.b, guid), words(&caps80));
    assert!(
        store.projection_context(id, B).unwrap().is_none()
            && !store.character_pin(id, B).unwrap().unwrap().projected
    );
    let c2 = store.load_current(id).unwrap();
    assert_eq!(c2, c1, "restoring the realm changed nothing canonical");

    // ---- and down again (cap 70, then 60): what the realm cannot hold is taken out of the realm, never out of the canonical character ----
    let hold70 = hold_at(&c2, 70, SIG70, &HIGH_SPELLS[2..]);
    let down = update_realm_character(
        &r.b,
        &mut store,
        id,
        B,
        &options(caps(70, SIG70), Some(hold70)),
    )
    .unwrap();
    assert!(down.updated);
    assert_eq!(level(&r.b, guid), 70);
    assert!(
        !has_item(&r.b, guid, HELMET),
        "still above the cap: out of the realm"
    );
    let on_realm = spells(&r.b, guid);
    assert!(
        on_realm.contains(&HIGH_SPELLS[0])
            && on_realm.contains(&HIGH_SPELLS[1])
            && !on_realm.contains(&HIGH_SPELLS[2]),
        "what a cap of 70 allows is on the realm, what it does not is not"
    );
    assert_eq!(
        store.load_current(id).unwrap(),
        c2,
        "the canonical character is untouched by a realm that holds less"
    );
    let ctx = store.projection_context(id, B).unwrap().unwrap();
    assert_eq!(
        (ctx.projected_level, ctx.progression_signature.as_str()),
        (70, SIG70)
    );

    // the update of a realm that is already where it should be writes nothing; a changed signature alone is enough to write
    let same = update_realm_character(
        &r.b,
        &mut store,
        id,
        B,
        &options(
            caps(70, SIG70),
            Some(hold_at(&c2, 70, SIG70, &HIGH_SPELLS[2..])),
        ),
    )
    .unwrap();
    assert!(!same.updated, "nothing moved: nothing is written");
    let moved = update_realm_character(
        &r.b,
        &mut store,
        id,
        B,
        &options(caps(60, SIG60), Some(hold_at(&c2, 60, SIG60, &HIGH_SPELLS))),
    )
    .unwrap();
    assert!(moved.updated);
    assert_eq!(level(&r.b, guid), 60);
    assert!(HIGH_SPELLS.iter().all(|s| !spells(&r.b, guid).contains(s)));
    assert_eq!(store.load_current(id).unwrap(), c2);
    assert_eq!(pin(&r.b, guid), words(&caps(60, SIG60)));
}
