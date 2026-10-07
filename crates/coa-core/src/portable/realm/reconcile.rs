//! Phase 4: the round trip. A character leaves canonical storage for a realm, is played there, and what was played comes back.
//!
//! ```text
//!   C0  canonical when the character joined the realm            (the realm's synced snapshot)
//!   B0  the realm's own first load/save of it, before any play   (captured by `begin_session`)
//!   B1  the realm now                                            (read by `reconcile_session`)
//!
//!   canonical' = merge3(target = C0, base = B0, ours = B1, Lenient)       only B0 -> B1 is progress
//!   realm'     = merge3(target = realm now, base = synced, ours = canonical', Strict)   in place, never re-created
//! ```
//!
//! Everything the realm did *by itself* before `B0` (default spells, skills and reputations, honor/title normalisation,
//! spells removed by the client data, items it mailed because they did not fit) is in `B0` and therefore never counts as
//! progress and never overwrites the canonical character. Everything realm-side that the canonical character does not own
//! (items the realm added; items it holds back) is tracked per mapping (`present` / `filtered` / `realm_local`) and
//! survives in both directions.
//!
//! All realm writes of this module are the in-place update of [`super::update`]; reconciling a session reads the realm
//! only.

use std::collections::HashMap;

use crate::db::Db;

use super::super::error::{PortableError, Result};
use super::super::ids::{CharacterId, ImportId, PortableItemId, PortablePetId, SessionId};
use super::super::merge::{merge3, ItemOutcome, Mode, PetOutcome};
use super::super::model::PortableCharacter;
use super::super::store::{BaselineInput, ImportAllocation, ImportState, JournalEntry, JournalKind, PlannedItem, PlannedPet, Store, UpdatePlan};
use super::import::{ImportOptions, ImportProblem, Resolution};
use super::plan::{SessionArm, IMPORT_LOCK};
use super::script::{parse_output, Query};
use super::sqlenc::Val;
use super::update::{build_update, new_content, parse_update_report, UpdateContext, UpdateCounts};
use super::{export_character_with_pets, probe, realm_error, ruleset_of};
use crate::portable::store::pet_identity;

fn local_guid(store: &Store, id: CharacterId, server_id: &str) -> Result<(u32, u64)> {
    let mapping = store
        .server_mappings(id)?
        .into_iter()
        .find(|m| m.server_id == server_id)
        .ok_or_else(|| PortableError::Invalid(format!("character {id} is not on realm {server_id}")))?;
    Ok((mapping.local_guid, mapping.last_revision))
}

/// What the realm shows of a mapped character: the model and the id tables the update needs.
struct RealmView {
    exported: super::Exported,
    items: HashMap<PortableItemId, u32>,
    pets: HashMap<PortablePetId, u32>,
}

fn read_realm(db: &Db, store: &Store, id: CharacterId, server_id: &str, local_guid: u32) -> Result<RealmView> {
    let prior_items = store.active_item_lookup(id, server_id)?;
    let prior_pets = store.active_pet_lookup(id, server_id)?;
    let exported = export_character_with_pets(db, local_guid, Some(id), &prior_items, &prior_pets)?;
    let items = exported.observations.iter().map(|o| (o.portable_item_id, o.local_item_guid)).collect();
    let pets = exported.pet_observations.iter().map(|o| (o.portable_pet_id, o.local_pet_number)).collect();
    Ok(RealmView { exported, items, pets })
}

// ---- the session baseline -----------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct SessionStart {
    /// The canonical revision the session joined at.
    pub c0_revision: u64,
    /// Items / pets the canonical character owns but the realm did not show at `B0` (filtered by the realm: they stay canonical).
    pub items_filtered: usize,
    pub pets_filtered: usize,
    /// Items / pets the realm showed at `B0` that the canonical character does not own (realm-local: never merged).
    pub items_realm_local: usize,
    pub pets_realm_local: usize,
    pub warnings: Vec<String>,
}

/// Freeze `B0`: the realm's state of this character after its own first load and save and before any play.
///
/// Call it **once per session, with the realm's first normalisation done and before the character is played**. The caller
/// (the Manager's join flow) owns that ordering; a baseline captured later would treat the progress made so far as the
/// realm's own normalisation and lose it.
pub fn begin_session(db: &Db, store: &mut Store, id: CharacterId, server_id: &str) -> Result<SessionStart> {
    let (guid, last_revision) = local_guid(store, id, server_id)?;
    if let Some(open) = store.open_imports(server_id)?.into_iter().find(|e| e.character_id == id) {
        return Err(PortableError::ImportInProgress { import_id: open.import_id });
    }
    if last_revision != store.character(id)?.revision {
        return Err(PortableError::StaleRevision { expected: last_revision, current: store.character(id)?.revision });
    }
    let view = read_realm(db, store, id, server_id, guid)?;
    let baseline = store.capture_baseline(id, server_id, BaselineInput { b0: &view.exported.model, items: &view.exported.observations, pets: &view.exported.pet_observations })?;
    let (c0_items, c0_pets) = (baseline.c0.items.iter().map(|i| i.id).collect::<std::collections::HashSet<_>>(), baseline.c0.pets.iter().map(|p| p.id).collect::<std::collections::HashSet<_>>());
    Ok(SessionStart {
        c0_revision: baseline.c0_revision,
        items_filtered: c0_items.iter().filter(|i| !view.items.contains_key(i)).count(),
        pets_filtered: c0_pets.iter().filter(|p| !view.pets.contains_key(p)).count(),
        items_realm_local: view.items.keys().filter(|i| !c0_items.contains(i)).count(),
        pets_realm_local: view.pets.keys().filter(|p| !c0_pets.contains(p)).count(),
        warnings: view.exported.warnings,
    })
}

// ---- reconciling a session back into canonical storage -----------------------------------------------------------------------

#[derive(Debug)]
pub struct ReconcileOutcome {
    /// The canonical revision after the reconcile (unchanged when the session changed nothing).
    pub revision: u64,
    pub new_revision: bool,
    /// What the session changed, as applied to the canonical character.
    pub changes: Vec<String>,
    /// What differs between the canonical character and the realm's `B0` and was deliberately not touched.
    pub left_alone: Vec<String>,
    pub items: ItemOutcome,
    pub pets: PetOutcome,
    pub warnings: Vec<String>,
}

/// Bring what was played on the realm back into the canonical character: `merge3(C0, B0, B1)`.
///
/// Reads the realm only (the character must be offline, as for every export). `close_session = false` keeps the baseline
/// open, which makes this a checkpoint; calling it again later applies the *whole* `B0 -> B1'` delta to `C0` again, so
/// repeating it never counts progress twice.
pub fn reconcile_session(db: &Db, store: &mut Store, id: CharacterId, server_id: &str, close_session: bool, note: Option<&str>) -> Result<ReconcileOutcome> {
    let baseline = store.open_baseline(id, server_id)?.ok_or(PortableError::NoBaseline)?;
    let (guid, _) = local_guid(store, id, server_id)?;
    if let Some(open) = store.open_imports(server_id)?.into_iter().find(|e| e.character_id == id) {
        return Err(PortableError::ImportInProgress { import_id: open.import_id });
    }
    let record = store.character(id)?;
    if record.revision != baseline.head_revision {
        return Err(PortableError::StaleRevision { expected: baseline.head_revision, current: record.revision });
    }
    let view = read_realm(db, store, id, server_id, guid)?;
    let merged = merge3(&baseline.c0, &baseline.b0, &view.exported.model, Mode::Lenient)?;
    let before = record.revision;
    let revision = store.commit_reconciled(id, server_id, merged.model, &view.exported.observations, &view.exported.pet_observations, note)?;
    if close_session {
        store.close_baseline(id, server_id)?;
    }
    Ok(ReconcileOutcome { revision, new_revision: revision != before, changes: merged.changes, left_alone: merged.left_alone, items: merged.items, pets: merged.pets, warnings: view.exported.warnings })
}

// ---- updating the realm's character in place ------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct UpdateOutcome {
    pub import_id: Option<ImportId>,
    pub from_revision: u64,
    pub to_revision: u64,
    /// `false` when the realm was already at the canonical revision: nothing was written.
    pub updated: bool,
    pub counts: UpdateCounts,
    pub changes: Vec<String>,
    pub left_alone: Vec<String>,
    pub warnings: Vec<String>,
}

fn update_problems(db: &Db, local_guid: u32, items: &std::collections::BTreeSet<u32>, creatures: &std::collections::BTreeSet<u32>, opts: &ImportOptions) -> Result<Vec<ImportProblem>> {
    let users = opts.game_server_users.iter().map(|u| Val::text(u.clone()).sql()).collect::<Vec<_>>().join(", ");
    let count = |name: &str, sql: String| Query { name: name.to_string(), columns: vec!["n"], sql };
    let ids = |set: &std::collections::BTreeSet<u32>| set.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
    let mut queries = vec![
        count("sessions", format!("SELECT COUNT(*) FROM information_schema.processlist WHERE id <> CONNECTION_ID() AND user IN ({users})")),
        count("online", "SELECT COUNT(*) FROM acore_characters.characters WHERE online <> 0".into()),
        count("character", format!("SELECT COUNT(*) FROM acore_characters.characters WHERE guid = {local_guid} AND deleteDate IS NULL")),
    ];
    if !items.is_empty() {
        queries.push(Query { name: "item_entries".into(), columns: vec!["entry"], sql: format!("SELECT entry FROM acore_world.item_template WHERE entry IN ({})", ids(items)) });
    }
    if !creatures.is_empty() {
        queries.push(Query { name: "creature_entries".into(), columns: vec!["entry"], sql: format!("SELECT entry FROM acore_world.creature_template WHERE entry IN ({})", ids(creatures)) });
    }
    let output = db.query(&super::script::snapshot_script(&queries)).map_err(realm_error)?;
    let raw = parse_output(&output, &queries)?;
    let one = |name: &str| -> Result<u64> { raw.section(name)?.iter().next().map(|r| r.u64("n")).transpose().map(|v| v.unwrap_or(0)) };
    let mut problems = Vec::new();
    if one("sessions")? > 0 {
        problems.push(ImportProblem::RealmRunning { sessions: one("sessions")? });
    }
    if one("online")? > 0 {
        problems.push(ImportProblem::OnlineCharacters(one("online")?));
    }
    if one("character")? == 0 {
        problems.push(ImportProblem::CharacterUnavailable(local_guid));
    }
    let known = |name: &str| -> Result<std::collections::BTreeSet<u32>> {
        if !raw.has(name) {
            return Ok(Default::default());
        }
        raw.section(name)?.iter().map(|r| r.u32("entry")).collect()
    };
    let missing: Vec<u32> = items.difference(&known("item_entries")?).copied().collect();
    if !missing.is_empty() {
        problems.push(ImportProblem::MissingItems(missing));
    }
    let missing: Vec<u32> = creatures.difference(&known("creature_entries")?).copied().collect();
    if !missing.is_empty() {
        problems.push(ImportProblem::MissingPetCreatures(missing));
    }
    Ok(problems)
}

/// Bring the character on `server_id` to the current canonical revision, **in place**: the same local character, its guid,
/// position, homebind and everything the model does not carry stay as they are; only the portable subset that differs is
/// written, by one transaction, journaled like an import.
///
/// Refused (nothing written) when a session is open on that realm (reconcile it first), when the realm is running, or when
/// the realm and the canonical character changed the same thing differently since they were last synchronised.
pub fn update_realm_character(db: &Db, store: &mut Store, id: CharacterId, server_id: &str, opts: &ImportOptions) -> Result<UpdateOutcome> {
    update_realm_character_in_session(db, store, id, server_id, opts, None)
}

/// The same, arming the runtime portable session `session` on the updated character in the same realm transaction.
pub fn update_realm_character_in_session(db: &Db, store: &mut Store, id: CharacterId, server_id: &str, opts: &ImportOptions, session: Option<SessionId>) -> Result<UpdateOutcome> {
    update_inner(db, store, id, server_id, opts, session, false)
}

/// What a re-evaluation did.
#[derive(Debug)]
pub struct Reevaluation {
    /// The realm's content profile is the one the character was last synchronised under: nothing was looked at.
    pub profile_unchanged: bool,
    /// The realm's character was written (held-back appearances that the realm can now show).
    pub update: Option<UpdateOutcome>,
    pub extensions: Vec<super::super::extension::ExtensionOutcome>,
}

/// The realm's content profile changed since this character was synchronised with it (a newer client data directory, a module added or
/// removed): look again at what was held back **even though the canonical revision did not change**. Only additions are made: an
/// appearance the realm can now show and does not have is written, a module payload that now has an adapter is applied; nothing the
/// realm itself has or changed since is touched. Afterwards the character carries the new profile hash.
///
/// The realm must be stopped, as for an update. Without capabilities in the options there is nothing to compare and it is refused.
pub fn reevaluate_realm_character(db: &Db, store: &mut Store, id: CharacterId, server_id: &str, opts: &ImportOptions) -> Result<Reevaluation> {
    let caps = opts.capabilities.clone().ok_or_else(|| PortableError::Invalid("re-evaluation needs the realm's content profile".into()))?;
    if store.mapping_profile(id, server_id)?.as_deref() == Some(caps.content_profile_hash.as_str()) {
        return Ok(Reevaluation { profile_unchanged: true, update: None, extensions: vec![] });
    }
    let update = update_inner(db, store, id, server_id, opts, None, true)?;
    let (guid, _) = local_guid(store, id, server_id)?;
    let canonical = store.load_current(id)?;
    let extensions = super::profile::apply_extensions(db, store, id, server_id, opts, guid, &canonical)?;
    store.set_mapping_profile(id, server_id, &caps.content_profile_hash)?;
    Ok(Reevaluation { profile_unchanged: false, update: update.updated.then_some(update), extensions })
}

/// The realm's character with the appearances the canonical character has, the realm can now show and the realm lacks added: a
/// category without a selection gets the canonical one, an outfit that is not there is added. What the realm has stays.
fn restored_wardrobe(current: &PortableCharacter, canonical: &PortableCharacter, knows: &dyn Fn(u32) -> bool) -> PortableCharacter {
    let mut out = current.clone();
    for (category, appearance) in &canonical.wardrobe.active {
        if knows(*appearance) && !out.wardrobe.active.contains_key(category) {
            out.wardrobe.active.insert(*category, *appearance);
        }
    }
    for (name, ids) in &canonical.wardrobe.outfits {
        if !out.wardrobe.outfits.contains_key(name) && ids.iter().all(|id| *id == 0 || knows(*id)) {
            out.wardrobe.outfits.insert(name.clone(), ids.clone());
        }
    }
    out
}

fn update_inner(db: &Db, store: &mut Store, id: CharacterId, server_id: &str, opts: &ImportOptions, session: Option<SessionId>, reevaluate: bool) -> Result<UpdateOutcome> {
    let (guid, last_revision) = local_guid(store, id, server_id)?;
    if store.open_baseline(id, server_id)?.is_some() {
        return Err(PortableError::SessionOpen);
    }
    if let Some(open) = store.open_imports(server_id)?.into_iter().find(|e| e.character_id == id) {
        return Err(PortableError::ImportInProgress { import_id: open.import_id });
    }
    let record = store.character(id)?;
    if reevaluate {
        if record.revision != last_revision {
            return Err(PortableError::Invalid("the realm is not at the canonical revision: update it first, then re-evaluate".into()));
        }
    } else if record.revision == last_revision {
        return Ok(UpdateOutcome { import_id: None, from_revision: last_revision, to_revision: last_revision, updated: false, counts: UpdateCounts::default(), changes: vec![], left_alone: vec![], warnings: vec![] });
    }
    if record.revision < last_revision {
        return Err(PortableError::StaleRevision { expected: last_revision, current: record.revision });
    }
    let synced = store.synced_model(id, server_id)?.ok_or_else(|| PortableError::Invalid("the realm has no synchronised snapshot to update from".into()))?;
    let canonical = store.load_current(id)?;
    let mut operations = vec![super::super::compat::Operation::Update];
    if session.is_some() {
        operations.push(super::super::compat::Operation::RuntimeSession);
    }
    let compatibility = super::profile::gate(opts, &canonical, &operations)?;
    let view = read_realm(db, store, id, server_id, guid)?;

    let merged = if reevaluate {
        let model = match opts.knowledge.as_deref() {
            Some(k) => restored_wardrobe(&view.exported.model, &canonical, &|id| k.knows_appearance(id)),
            None => view.exported.model.clone(),
        };
        let changes = if model.wardrobe != view.exported.model.wardrobe { vec!["wardrobe: appearances the realm can now show were restored".to_string()] } else { vec![] };
        if changes.is_empty() {
            return Ok(UpdateOutcome { import_id: None, from_revision: last_revision, to_revision: last_revision, updated: false, counts: UpdateCounts::default(), changes: vec![], left_alone: vec![], warnings: vec![] });
        }
        super::super::merge::Merged { model, changes, left_alone: vec![], conflicts: vec![], items: Default::default(), pets: Default::default() }
    } else {
        let merged = merge3(&view.exported.model, &synced, &canonical, Mode::Strict)?;
        if !merged.conflicts.is_empty() {
            return Err(PortableError::UpdateConflicts(merged.conflicts.iter().map(|c| format!("{}: {}", c.path, c.detail)).collect()));
        }
        merged
    };
    let schema = probe(db)?;
    let context = |nonce| UpdateContext {
        ruleset: ruleset_of(db),
        local_guid: guid,
        revision: record.revision,
        nonce,
        game_server_users: &opts.game_server_users,
        probe: &schema,
        items: &view.items,
        pets: &view.pets,
        session: session.map(|session_id| SessionArm { session_id, character_id: id, generation: 1 }),
        knowledge: opts.knowledge.as_deref(),
    };
    // the plan (what is added, what is removed) does not depend on the nonce
    let draft = build_update(&view.exported.model, &merged.model, &context([0; 4]))?;
    let (new_items, new_creatures) = new_content(&merged.model, &draft.added_items, &draft.added_pets);
    let problems = update_problems(db, guid, &new_items, &new_creatures, opts)?;
    if !problems.is_empty() {
        return Err(PortableError::ImportRefused(problems));
    }

    let by_id_item: HashMap<_, _> = merged.model.items.iter().map(|i| (i.id, i)).collect();
    let by_id_pet: HashMap<_, _> = merged.model.pets.iter().map(|p| (p.id, p)).collect();
    let plan = UpdatePlan {
        added_items: draft.added_items.iter().map(|id| PlannedItem { id: *id, entry: by_id_item[id].entry.clone(), identity: super::super::identity::item_identity(&by_id_item[id].entry, by_id_item[id].random_property_id) }).collect(),
        added_pets: draft.added_pets.iter().map(|id| PlannedPet { id: *id, entry: by_id_pet[id].entry.clone(), identity: pet_identity(&by_id_pet[id].entry, by_id_pet[id].pet_type, by_id_pet[id].created_by_spell) }).collect(),
        retired_items: draft.removed_items.clone(),
        retired_pets: draft.removed_pets.clone(),
    };
    let ticket = store.begin_update(id, server_id, record.revision, plan)?;
    let script = match build_update(&view.exported.model, &merged.model, &context(ticket.nonce)) {
        Ok(s) => s,
        Err(e) => {
            store.abort_import(ticket.import_id, &format!("the update could not be built: {e}"))?;
            return Err(e);
        }
    };
    debug_assert_eq!(script.marker_data, ticket.marker);

    let reported = db.query(&script.script).map_err(realm_error).and_then(|out| parse_update_report(&out));
    match reported {
        Ok(((g, item_base, pet_base), true)) if g == guid => {
            store.finish_import(ticket.import_id, ImportAllocation { local_guid: g, item_base, pet_base })?;
        }
        outcome => {
            let original = outcome.err();
            match resolve_import_update(db, store, ticket.import_id, opts, false)? {
                Resolution::Committed(_) => {}
                Resolution::Aborted => return Err(original.unwrap_or_else(|| PortableError::RealmRead("the realm did not commit the update".into()))),
                Resolution::Pending(why) => {
                    return Err(PortableError::ImportNeedsAttention { import_id: ticket.import_id, detail: format!("the outcome in the realm is not known yet ({why}); run recovery") })
                }
            }
        }
    }
    let mut warnings = view.exported.warnings;
    warnings.extend(super::profile::after_write(Some(db), store, id, server_id, opts, guid, &canonical, compatibility.as_ref())?);
    Ok(UpdateOutcome {
        import_id: Some(ticket.import_id),
        from_revision: last_revision,
        to_revision: record.revision,
        updated: true,
        counts: script.counts,
        changes: merged.changes,
        left_alone: merged.left_alone,
        warnings,
    })
}

// ---- recovery of an interrupted update -------------------------------------------------------------------------------------------

fn recovery_script(local_guid: u32, wait: u32) -> String {
    let marker = Val::text(super::policy::IMPORT_MARKER_SOURCE).sql();
    let alloc = Val::text(super::policy::ALLOC_MARKER_SOURCE).sql();
    format!(
        "SELECT '#T:lock';\nSELECT GET_LOCK('{IMPORT_LOCK}', {wait});\n\
         SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ;\nSTART TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY;\n\
         SELECT '#T:marker';\nSELECT IFNULL((SELECT data FROM acore_characters.character_settings WHERE guid = {local_guid} AND source = {marker}), 'none');\n\
         SELECT '#T:alloc';\nSELECT IFNULL((SELECT data FROM acore_characters.character_settings WHERE guid = {local_guid} AND source = {alloc}), 'none');\n\
         COMMIT;\nDO RELEASE_LOCK('{IMPORT_LOCK}');\n"
    )
}

/// Decide what became of one unfinished **update** and bring the local records in line with the realm. The marker row the
/// update wrote inside its transaction is the proof: present with this update's nonce = committed.
pub fn resolve_import_update(db: &Db, store: &mut Store, import_id: ImportId, opts: &ImportOptions, with_grace: bool) -> Result<Resolution> {
    let entry: JournalEntry = store.import_entry(import_id)?;
    if entry.kind != JournalKind::Update {
        return Err(PortableError::Invalid("this journal entry is not an update".into()));
    }
    match entry.state {
        ImportState::Committed => return Ok(Resolution::Committed(entry.allocation.expect("a committed entry has its allocation"))),
        ImportState::Aborted => return Ok(Resolution::Aborted),
        ImportState::NeedsAttention => return Err(PortableError::ImportNeedsAttention { import_id, detail: entry.detail.unwrap_or_default() }),
        ImportState::Prepared => {}
    }
    let (guid, _) = local_guid(store, entry.character_id, &entry.server_id)?;
    let queries = vec![
        Query { name: "lock".into(), columns: vec!["n"], sql: String::new() },
        Query { name: "marker".into(), columns: vec!["data"], sql: String::new() },
        Query { name: "alloc".into(), columns: vec!["data"], sql: String::new() },
    ];
    let output = db.query(&recovery_script(guid, opts.lock_wait_seconds)).map_err(realm_error)?;
    let raw = parse_output(&output, &queries)?;
    let cell = |name: &str| -> Result<String> { Ok(raw.section(name)?.iter().next().ok_or_else(|| PortableError::CorruptSnapshot(format!("no {name} answer")))?.plain("data")?.trim().to_string()) };
    if raw.section("lock")?.iter().next().ok_or_else(|| PortableError::CorruptSnapshot("no lock answer".into()))?.u64("n")? != 1 {
        return Ok(Resolution::Pending("an import is still running in the realm".into()));
    }
    if cell("marker")? != entry.marker.trim() {
        if with_grace && age_seconds(&entry.created_at) < opts.recovery_grace.as_secs() as i64 {
            return Ok(Resolution::Pending("the update is too recent to be called lost".into()));
        }
        store.abort_import(import_id, "the realm has no trace of this update: it never committed")?;
        return Ok(Resolution::Aborted);
    }
    let alloc_text = cell("alloc")?;
    let words: Vec<&str> = alloc_text.split_whitespace().collect();
    let (item_base, pet_base) = match words.as_slice() {
        [i, p] => (i.parse::<u32>().ok(), p.parse::<u32>().ok()),
        _ => (None, None),
    };
    let (Some(item_base), Some(pet_base)) = (item_base, pet_base) else {
        let detail = format!("the update's marker is in the realm but its allocation row is unreadable ({alloc_text:?})");
        store.flag_import(import_id, &detail)?;
        return Err(PortableError::ImportNeedsAttention { import_id, detail });
    };
    // the items and pets the update allocated must be there, contiguous from the recorded bases
    let present = |sql: String| -> Result<u64> {
        db.query(&sql).map_err(realm_error)?.lines().next().unwrap_or("").trim().parse().map_err(|_| PortableError::CorruptSnapshot("unexpected recovery answer".into()))
    };
    let (n_items, n_pets) = (entry.items.len() as u64, entry.pets.len() as u64);
    let items_found = if n_items == 0 { 0 } else { present(format!("SELECT COUNT(*) FROM acore_characters.item_instance WHERE owner_guid = {guid} AND guid >= {item_base} AND guid < {item_base} + {n_items}"))? };
    let pets_found = if n_pets == 0 { 0 } else { present(format!("SELECT COUNT(*) FROM acore_characters.character_pet WHERE owner = {guid} AND id >= {pet_base} AND id < {pet_base} + {n_pets}"))? };
    if items_found != n_items || pets_found != n_pets {
        let detail = format!("the realm carries this update's marker but holds {items_found} of {n_items} new items and {pets_found} of {n_pets} new pets");
        store.flag_import(import_id, &detail)?;
        return Err(PortableError::ImportNeedsAttention { import_id, detail });
    }
    let alloc = ImportAllocation { local_guid: guid, item_base, pet_base };
    store.finish_import(import_id, alloc)?;
    Ok(Resolution::Committed(alloc))
}

fn age_seconds(created_at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(created_at).map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds()).unwrap_or(i64::MAX)
}

/// Re-exported for callers that only hold a merged result and want the counts without running anything.
pub fn summarize(merged: &PortableCharacter, current: &PortableCharacter) -> Result<UpdateCounts> {
    let items: HashMap<PortableItemId, u32> = current.items.iter().enumerate().map(|(i, it)| (it.id, i as u32 + 1)).collect();
    let pets: HashMap<PortablePetId, u32> = current.pets.iter().enumerate().map(|(i, p)| (p.id, i as u32 + 1)).collect();
    let users = ["acore".to_string()];
    let schema = super::script::SchemaProbe::default();
    Ok(build_update(current, merged, &UpdateContext { ruleset: current.ruleset, local_guid: 1, revision: 1, nonce: [0; 4], game_server_users: &users, probe: &schema, items: &items, pets: &pets, session: None, knowledge: None })?.counts)
}
