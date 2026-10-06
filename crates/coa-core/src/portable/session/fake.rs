//! A simulated realm for the session tests: the core's marker, gate, checkpoint and logout behaviour, and a realm that normalises
//! characters on its own. It is deliberately small; the real worldserver is exercised by `live` tests.

use std::collections::HashMap;

use super::super::collection::IdSet;
use super::super::error::{PortableError, Result};
use super::super::identity::item_identity;
use super::super::ids::{CharacterId, ContentId, PortableItemId, PortablePetId, SessionId};
use super::super::model::*;
use super::super::realm::collections::Applied;
use super::super::realm::knowledge::RealmKnowledge;
use super::super::realm::Exported;
use super::super::store::{pet_identity, ImportAllocation, ItemObservation, PetObservation, PlannedItem, PlannedPet, Store};
use super::bridge::*;

pub struct FakeRealm {
    pub guid: u32,
    /// What the player is playing right now.
    pub memory: PortableCharacter,
    /// What the realm's tables hold (the last save).
    pub db: PortableCharacter,
    item_guid: HashMap<PortableItemId, u32>,
    pet_number: HashMap<PortablePetId, u32>,
    next_item_guid: u32,
    next_pet_number: u32,
    pub row: Option<SessionRow>,
    pub online: bool,
    /// The core holds the player until the Host releases the session.
    pub gated: bool,
    /// Checkpoint requests that answer `Busy` before one succeeds.
    pub busy: u32,
    pub saves: u32,
    pub released: Vec<SessionId>,
    pub normalize: fn(&mut FakeRealm),
    /// The game account of the character and its two collection tables.
    pub account: u32,
    pub appearance_rows: IdSet,
    pub vanity_rows: IdSet,
    /// The realm's client data: what it can show.
    pub knowledge: RealmKnowledge,
    /// How often the collections were fingerprinted, read in full and written (the tests prove "nothing changed, nothing read").
    pub fingerprints: u32,
    pub full_reads: u32,
    pub collection_writes: u32,
}

pub fn new_internal_item(n: u64, slot: u8, entry: u64) -> PortableItem {
    PortableItem {
        id: PortableItemId::from_uuid(crate::portable::fixtures::id7(7_000_000 + n)).unwrap(),
        container: None,
        slot,
        entry: ContentId::new("coa", "item", entry).unwrap(),
        count: 1,
        duration: 0,
        charges: vec![],
        flags: 0,
        enchantments: vec![],
        random_property_id: 0,
        durability: 30,
        played_time: 0,
        text: None,
        creator_name: None,
        gift: None,
    }
}

impl FakeRealm {
    pub fn new(guid: u32) -> Self {
        let empty = crate::portable::fixtures::naked_level_one();
        Self {
            guid,
            memory: empty.clone(),
            db: empty,
            item_guid: HashMap::new(),
            pet_number: HashMap::new(),
            next_item_guid: 50_000,
            next_pet_number: 900,
            row: None,
            online: false,
            gated: false,
            busy: 0,
            saves: 0,
            released: vec![],
            normalize: |_| {},
            account: 1,
            appearance_rows: IdSet::new(),
            vanity_rows: IdSet::new(),
            knowledge: RealmKnowledge::default(),
            fingerprints: 0,
            full_reads: 0,
            collection_writes: 0,
        }
    }

    /// The character arrives (an import): items and pets get local ids in plan order, like the offline importer does.
    pub fn import(&mut self, store: &mut Store, character: CharacterId, server_id: &str, revision: u64, session: SessionId) -> Result<()> {
        let model = store.load_snapshot(character, revision)?;
        let items: Vec<PlannedItem> = model.items.iter().map(|i| PlannedItem { id: i.id, entry: i.entry.clone(), identity: item_identity(&i.entry, i.random_property_id) }).collect();
        let pets: Vec<PlannedPet> = model.pets.iter().map(|p| PlannedPet { id: p.id, entry: p.entry.clone(), identity: pet_identity(&p.entry, p.pet_type, p.created_by_spell) }).collect();
        let ticket = store.begin_import(character, server_id, revision, &items, &pets)?;
        let (item_base, pet_base) = (self.next_item_guid, self.next_pet_number);
        self.next_item_guid += items.len() as u32;
        self.next_pet_number += pets.len() as u32;
        for (i, item) in items.iter().enumerate() {
            self.item_guid.insert(item.id, item_base + i as u32);
        }
        for (i, pet) in pets.iter().enumerate() {
            self.pet_number.insert(pet.id, pet_base + i as u32);
        }
        self.db = model.clone();
        self.memory = model;
        store.finish_import(ticket.import_id, ImportAllocation { local_guid: self.guid, item_base, pet_base })?;
        self.row = Some(SessionRow { guid: self.guid, session_id: session, character_id: character, imported_revision: revision, generation: 1, state: RowState::WaitingBaseline, checkpoint_seq: 0, save_seq: 0 });
        Ok(())
    }

    fn free_slot(&self) -> u8 {
        (23u8..=38).chain(39..=66).find(|s| !self.memory.items.iter().any(|i| i.container.is_none() && i.slot == *s)).expect("a free slot")
    }

    /// The realm gives the character an item; it goes to the first free place.
    pub fn give_item(&mut self, mut item: PortableItem) -> u32 {
        item.slot = self.free_slot();
        let guid = self.next_item_guid;
        self.next_item_guid += 1;
        self.item_guid.insert(item.id, guid);
        self.memory.items.push(item);
        guid
    }

    fn save(&mut self) {
        self.db = self.memory.clone().normalized();
        self.saves += 1;
        if let Some(r) = &mut self.row {
            r.save_seq += 1;
        }
    }

    /// The player logs in: the realm normalises, and a portable character in `WaitingBaseline` is saved and held.
    pub fn login(&mut self) {
        assert!(!self.online);
        self.online = true;
        self.memory = self.db.clone();
        (self.normalize)(self);
        let waiting = matches!(&self.row, Some(r) if matches!(r.state, RowState::WaitingBaseline | RowState::BaselineReady));
        if waiting {
            self.save();
            let row = self.row.as_mut().unwrap();
            row.state = RowState::BaselineReady;
            self.gated = true;
        } else if let Some(r) = &mut self.row {
            if r.state == RowState::Ended {
                r.state = RowState::Active;
            }
        }
    }

    /// Gameplay. It is a test failure to play before the baseline was released: that is the property the gate guarantees.
    pub fn play(&mut self, f: impl FnOnce(&mut PortableCharacter)) {
        assert!(self.online, "the player is not online");
        assert!(!self.gated, "gameplay happened before B0 was taken");
        f(&mut self.memory);
    }

    pub fn autosave(&mut self) {
        assert!(self.online);
        self.save();
    }

    pub fn logout(&mut self) {
        assert!(self.online);
        self.save();
        self.online = false;
        self.gated = false;
        if let Some(r) = &mut self.row {
            r.state = RowState::Ended;
        }
    }

    /// The worldserver or the client dies: the last save stays, nothing else.
    pub fn crash(&mut self) {
        self.online = false;
        self.gated = false;
        self.memory = self.db.clone();
    }

    /// The gate's timeout: the player is disconnected, never released.
    pub fn gate_timeout(&mut self) {
        assert!(self.gated);
        self.online = false;
        self.gated = false;
    }
}

impl RealmBridge for FakeRealm {
    fn session_row(&mut self, local_guid: u32) -> Result<Option<SessionRow>> {
        Ok(if local_guid == self.guid { self.row.clone() } else { None })
    }

    fn read(&mut self, local_guid: u32, prior_items: &HashMap<u32, (PortableItemId, String)>, prior_pets: &HashMap<u32, (PortablePetId, String)>) -> Result<RealmRead> {
        assert_eq!(local_guid, self.guid);
        let mut model = self.db.clone();
        let mut ids: HashMap<PortableItemId, PortableItemId> = HashMap::new();
        let mut observations = Vec::new();
        for item in &mut model.items {
            let internal = item.id;
            let guid = *self.item_guid.get(&internal).ok_or_else(|| PortableError::Invalid("the simulated realm lost track of an item".into()))?;
            let identity = item_identity(&item.entry, item.random_property_id);
            let id = match prior_items.get(&guid) {
                Some((id, prior)) if *prior == identity => *id,
                _ => PortableItemId::new(),
            };
            ids.insert(internal, id);
            item.id = id;
            observations.push(ItemObservation { portable_item_id: id, local_item_guid: guid, entry: item.entry.clone(), identity });
        }
        for item in &mut model.items {
            item.container = item.container.and_then(|c| ids.get(&c).copied());
        }
        let mut pet_observations = Vec::new();
        for pet in &mut model.pets {
            let internal = pet.id;
            let number = *self.pet_number.entry(internal).or_insert_with(|| {
                self.next_pet_number += 1;
                self.next_pet_number
            });
            let identity = pet_identity(&pet.entry, pet.pet_type, pet.created_by_spell);
            let id = match prior_pets.get(&number) {
                Some((id, prior)) if *prior == identity => *id,
                _ => PortablePetId::new(),
            };
            pet.id = id;
            pet_observations.push(PetObservation { portable_pet_id: id, local_pet_number: number, identity });
        }
        // the realm's tables are keyed by guid: a later read finds the same rows under the same internal ids
        let model = model.normalized();
        Ok(RealmRead { exported: Exported { model, local_guid: self.guid, account: 1, observations, pet_observations, session: self.row.clone(), warnings: vec![] }, session: self.row.clone(), online: self.online })
    }

    fn request_checkpoint(&mut self, local_guid: u32, session: SessionId, sequence: u64) -> Result<CheckpointReply> {
        if local_guid != self.guid || !self.online {
            return Ok(CheckpointReply::NotOnline);
        }
        let Some(row) = &self.row else { return Ok(CheckpointReply::Refused("not a portable character".into())) };
        if row.session_id != session || row.state != RowState::Active {
            return Ok(CheckpointReply::Refused("wrong session".into()));
        }
        if sequence <= row.checkpoint_seq {
            return Ok(CheckpointReply::Refused("stale sequence".into()));
        }
        if self.busy > 0 {
            self.busy -= 1;
            return Ok(CheckpointReply::Busy);
        }
        self.save();
        self.row.as_mut().unwrap().checkpoint_seq = sequence;
        Ok(CheckpointReply::Queued)
    }

    fn release(&mut self, session: SessionId) -> Result<()> {
        if let Some(r) = &mut self.row {
            if r.session_id == session && r.state == RowState::BaselineReady {
                r.state = RowState::Active;
                self.gated = false;
                self.released.push(session);
            }
        }
        Ok(())
    }

    fn arm(&mut self, local_guid: u32, session: SessionId, character: CharacterId, revision: u64, generation: u32) -> Result<()> {
        assert_eq!(local_guid, self.guid);
        assert!(!self.online, "a character is re-armed while it is offline");
        let save_seq = self.row.as_ref().map_or(0, |r| r.save_seq);
        self.row = Some(SessionRow { guid: local_guid, session_id: session, character_id: character, imported_revision: revision, generation, state: RowState::WaitingBaseline, checkpoint_seq: 0, save_seq });
        Ok(())
    }

    fn account_of(&mut self, local_guid: u32) -> Result<Option<u32>> {
        Ok((local_guid == self.guid).then_some(self.account))
    }

    fn collection_fingerprint(&mut self, account: u32, kind: &str) -> Result<String> {
        assert_eq!(account, self.account);
        self.fingerprints += 1;
        let set = self.rows(kind)?;
        Ok(format!("{}:{}:{}", set.len(), set.ids().last().copied().unwrap_or(0), set.ids().iter().map(|i| *i as u64).sum::<u64>()))
    }

    fn read_collection(&mut self, account: u32, kind: &str) -> Result<IdSet> {
        assert_eq!(account, self.account);
        self.full_reads += 1;
        Ok(self.rows(kind)?.clone())
    }

    fn apply_collection(&mut self, account: u32, kind: &str, canonical: &IdSet) -> Result<Applied> {
        assert_eq!(account, self.account);
        let knows = |id: u32| if kind == "coa:appearance" { self.knowledge.knows_appearance(id) } else { self.knowledge.knows_vanity(id) };
        let held = self.rows(kind)?.clone();
        let mut applied = Applied::default();
        let mut add = Vec::new();
        for id in canonical.ids().iter().copied() {
            if !knows(id) {
                applied.unknown += 1;
            } else if held.contains(id) {
                applied.already += 1;
            } else {
                add.push(id);
            }
        }
        applied.inserted = add.len();
        if !add.is_empty() {
            self.collection_writes += 1;
            let union = held.union(&IdSet::from_ids(add)?);
            *self.rows_mut(kind)? = union;
        }
        Ok(applied)
    }
}

impl FakeRealm {
    fn rows(&self, kind: &str) -> Result<&IdSet> {
        match kind {
            "coa:appearance" => Ok(&self.appearance_rows),
            "coa:vanity" => Ok(&self.vanity_rows),
            other => Err(PortableError::Invalid(format!("unknown kind {other}"))),
        }
    }

    fn rows_mut(&mut self, kind: &str) -> Result<&mut IdSet> {
        match kind {
            "coa:appearance" => Ok(&mut self.appearance_rows),
            "coa:vanity" => Ok(&mut self.vanity_rows),
            other => Err(PortableError::Invalid(format!("unknown kind {other}"))),
        }
    }

    /// The player unlocks ids (the core `INSERT IGNORE`s them).
    pub fn unlock(&mut self, kind: &str, ids: impl IntoIterator<Item = u32>) {
        let union = self.rows(kind).unwrap().union(&IdSet::from_ids(ids).unwrap());
        *self.rows_mut(kind).unwrap() = union;
    }
}
