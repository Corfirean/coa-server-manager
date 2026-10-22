//! Phase 7 on the simulated realm: a change of the realm's content profile is noticed and what was held back is looked at again, and
//! module extensions travel opaque through the Owner, are exported only by an adapter of the realm, and are never destroyed.

use super::fake::*;
use super::*;
use crate::portable::capabilities::*;
use crate::portable::collection::IdSet;
use crate::portable::extension::fake::{payload, FakeAdapter};
use crate::portable::extension::ExtensionRegistry;
use crate::portable::fixtures::geared_level_eighty;
use crate::portable::ids::CharacterId;
use crate::portable::model::{PortableCharacter, Ruleset};
use crate::portable::realm::knowledge::RealmKnowledge;
use crate::portable::store::Store;

const SERVER: &str = "realm-b";
const GUID: u32 = 4242;
const APPEARANCE: &str = "coa:appearance";

fn caps(catalog_marker: u8) -> RealmCapabilities {
    let mut catalog = ClientCatalog::new();
    catalog.insert(
        "Appearances.dbc".into(),
        CatalogEntry {
            sha256: format!("{catalog_marker:02x}").repeat(32),
            records: 500,
        },
    );
    RealmCapabilities::new(
        None,
        ContentProfile {
            ruleset: Ruleset::Coa,
            character_formats: ContentProfile::manager_formats(),
            online_import_job_formats: vec![2],
            session_protocol: 1,
            collection_protocol: 1,
            features: [
                Feature::RuntimeSessions,
                Feature::Wardrobe,
                Feature::Collections,
            ]
            .into_iter()
            .collect(),
            collection_kinds: ["coa:appearance".to_string(), "coa:vanity".to_string()]
                .into_iter()
                .collect(),
            extensions: vec![],
            client_catalog: catalog,
        },
    )
    .unwrap()
}

struct Rig {
    owner: Store,
    host: Store,
    realm: FakeRealm,
    id: CharacterId,
}

fn rig(model: impl FnOnce(&mut PortableCharacter)) -> Rig {
    let mut owner = Store::open_in_memory().unwrap();
    let mut host = Store::open_in_memory().unwrap();
    let profile = owner.default_profile().unwrap();
    let mut m = geared_level_eighty();
    model(&mut m);
    let id = owner.create_character(profile, m, "realm-a").unwrap();
    let offer = OwnerService::new(&mut owner).offer(id, SERVER).unwrap();
    let host_profile = host.default_profile().unwrap();
    let mut realm = FakeRealm::new(GUID);
    HostService::new(&mut host, SERVER, HostConfig::default())
        .accept_offer(host_profile, &offer)
        .unwrap();
    realm
        .import(
            &mut host,
            id,
            SERVER,
            offer.canonical_revision,
            offer.session_id,
        )
        .unwrap();
    HostService::new(&mut host, SERVER, HostConfig::default())
        .bind(offer.session_id, GUID)
        .unwrap();
    Rig {
        owner,
        host,
        realm,
        id,
    }
}

fn deliver_all(h: &mut HostService, o: &mut OwnerService, realm: &mut FakeRealm) {
    for m in h.outbox().unwrap() {
        let ack = if m.started {
            o.handle_started(&m.bytes)
        } else {
            o.handle_checkpoint(&m.bytes)
        }
        .unwrap();
        h.receive_ack(realm, &ack).unwrap();
    }
    for m in h.collection_outbox().unwrap() {
        let ack = o.handle_collection(&m.bytes).unwrap();
        h.receive_collection_ack(realm, m.account, &ack).unwrap();
    }
}

fn start(h: &mut HostService, o: &mut OwnerService, realm: &mut FakeRealm) {
    realm.login();
    h.tick(realm, 0).unwrap();
    deliver_all(h, o, realm);
}

fn ids(range: std::ops::RangeInclusive<u32>) -> IdSet {
    IdSet::from_ids(range).unwrap()
}

#[test]
fn a_changed_content_profile_makes_the_realm_report_its_collections_again_and_receive_what_it_can_now_show_without_a_new_revision(
) {
    let Rig {
        mut owner,
        mut host,
        mut realm,
        id,
    } = rig(|_| {});
    let profile = owner.default_profile().unwrap();
    owner
        .merge_collection(profile, APPEARANCE, &ids(1..=500))
        .unwrap();
    // the realm's client data knows 1..=300 of the canonical 500
    realm.knowledge = RealmKnowledge::new(ids(1..=300), IdSet::new());
    let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
    let mut o = OwnerService::new(&mut owner);
    let first = caps(1);
    let (change, stale) = h.observe_profile(&first, "live").unwrap();
    assert!(change.changed() && change.previous.is_none());
    assert_eq!(
        stale.len(),
        1,
        "a character synchronised before profiles existed is stale"
    );
    start(&mut h, &mut o, &mut realm);
    assert_eq!(
        realm.appearance_rows.len(),
        300,
        "what the realm knows was written, the rest stays canonical"
    );
    let revision = o.store().character(id).unwrap().revision;
    let reads = realm.full_reads;

    // the same profile again: nothing is forgotten, nothing is read
    let (change, _) = h.observe_profile(&first, "live").unwrap();
    assert!(!change.changed());
    h.tick(&mut realm, 10).unwrap();
    assert_eq!(realm.full_reads, reads);

    // the realm gets newer client data: another catalog hash, and it now knows all 500
    realm.knowledge = RealmKnowledge::new(ids(1..=500), IdSet::new());
    let second = caps(2);
    assert_ne!(first.content_profile_hash, second.content_profile_hash);
    let (change, _) = h.observe_profile(&second, "live").unwrap();
    assert_eq!(
        change.previous.as_deref(),
        Some(first.content_profile_hash.as_str())
    );
    let events = h.tick(&mut realm, 20).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HostEvent::CollectionQueued { kind, .. } if kind == APPEARANCE)),
        "the account is reported again: {events:?}"
    );
    deliver_all(&mut h, &mut o, &mut realm);
    assert_eq!(
        realm.appearance_rows.len(),
        500,
        "the 200 that were held back are written now"
    );
    assert_eq!(
        o.store().character(id).unwrap().revision,
        revision,
        "the canonical revision did not change"
    );
    assert_eq!(
        o.store()
            .collection(profile, APPEARANCE)
            .unwrap()
            .unwrap()
            .1
            .len(),
        500,
        "and nothing was lost or doubled in the Owner"
    );
}

#[test]
fn the_profile_hash_is_remembered_per_realm_and_per_character() {
    let Rig { mut host, id, .. } = rig(|_| {});
    let a = caps(1);
    let change = host.set_realm_profile(SERVER, &a, "live").unwrap();
    assert!(change.changed());
    assert_eq!(host.realm_profile(SERVER).unwrap().unwrap(), a);
    assert!(host.realm_profile("realm-z").unwrap().is_none());
    assert!(
        !host
            .set_realm_profile(SERVER, &a, "offline")
            .unwrap()
            .changed(),
        "the same content, whatever its source"
    );
    assert_eq!(host.mapping_profile(id, SERVER).unwrap(), None);
    host.set_mapping_profile(id, SERVER, &a.content_profile_hash)
        .unwrap();
    assert!(host
        .mappings_with_other_profile(SERVER, &a.content_profile_hash)
        .unwrap()
        .is_empty());
    let b = caps(2);
    assert_eq!(
        host.mappings_with_other_profile(SERVER, &b.content_profile_hash)
            .unwrap()
            .len(),
        1
    );
    assert!(
        host.set_mapping_profile(id, "realm-z", "x").is_err(),
        "a character that is not on the realm has no profile to record"
    );
    assert!(host
        .set_realm_profile("not a server id!", &a, "live")
        .is_err());
}

fn fake_registry() -> (ExtensionRegistry, std::sync::Arc<FakeAdapter>) {
    let adapter = FakeAdapter::new(2, 3);
    let mut r = ExtensionRegistry::new();
    r.register(adapter.clone()).unwrap();
    (r, adapter)
}

#[test]
fn an_extension_the_realm_cannot_read_survives_its_sessions_and_a_realm_with_the_module_gets_it_and_hands_changes_back(
) {
    let (registry, _adapter) = fake_registry();
    // the canonical character carries a module payload from an earlier realm
    let Rig {
        mut owner,
        mut host,
        mut realm,
        id,
    } = rig(|m| {
        m.extensions
            .insert("mod:fake".into(), payload(2, b"from the first realm"));
        m.extensions.insert(
            "mod:unknown-module".into(),
            payload(9, b"opaque, nobody here understands it"),
        );
    });
    let original = owner.load_current(id).unwrap().extensions.clone();
    realm.db.extensions.clear();

    // this realm has no module: B0 and B1 hold no module data, the session changes the money and nothing else
    {
        let mut h = HostService::new(&mut host, SERVER, HostConfig::default());
        let mut o = OwnerService::new(&mut owner);
        start(&mut h, &mut o, &mut realm);
        realm.play(|m| m.progression.money += 5);
        h.tick(&mut realm, 100).unwrap();
        h.tick(&mut realm, 101).unwrap();
        deliver_all(&mut h, &mut o, &mut realm);
        realm.logout();
        h.tick(&mut realm, 200).unwrap();
        deliver_all(&mut h, &mut o, &mut realm);
    }
    let after = owner.load_current(id).unwrap();
    assert_eq!(
        after.extensions, original,
        "no payload was destroyed, changed or applied by a realm that does not understand them"
    );
    assert_eq!(
        after.progression.money,
        geared_level_eighty().progression.money + 5
    );

    // a later realm that has the module: the payload is applied, and its own changes come back as a new canonical payload
    let (mut owner2, mut host2) = (owner, Store::open_in_memory().unwrap());
    let offer = OwnerService::new(&mut owner2).offer(id, "realm-c").unwrap();
    let hp = host2.default_profile().unwrap();
    let mut realm2 = FakeRealm::new(GUID);
    realm2.extension_realm.has_module = true;
    realm2.registry = Some(registry.clone());
    HostService::new(&mut host2, "realm-c", HostConfig::default())
        .accept_offer(hp, &offer)
        .unwrap();
    realm2
        .import(
            &mut host2,
            id,
            "realm-c",
            offer.canonical_revision,
            offer.session_id,
        )
        .unwrap();
    HostService::new(&mut host2, "realm-c", HostConfig::default())
        .bind(offer.session_id, GUID)
        .unwrap();
    let mut with_module = caps(1).content;
    with_module.extensions = {
        let mut r = crate::portable::extension::fake::FakeExtensionRealm {
            guid: GUID,
            has_module: true,
            ..Default::default()
        };
        registry.supported_on(&mut r)
    };
    let profile = RealmCapabilities::new(None, with_module).unwrap();
    let canonical = owner2.load_current(id).unwrap();
    let applied = registry.apply_all(
        &mut realm2.extension_realm,
        &canonical.extensions,
        &profile.content,
        &Default::default(),
    );
    assert_eq!(
        applied
            .iter()
            .find(|o| o.namespace == "mod:fake")
            .unwrap()
            .disposition,
        crate::portable::extension::ExtensionDisposition::Applied
    );
    assert!(matches!(
        applied
            .iter()
            .find(|o| o.namespace == "mod:unknown-module")
            .unwrap()
            .disposition,
        crate::portable::extension::ExtensionDisposition::Deferred(_)
    ));
    assert_eq!(realm2.extension_realm.data[&GUID], b"from the first realm");

    {
        let mut h = HostService::new(&mut host2, "realm-c", HostConfig::default());
        let mut o = OwnerService::new(&mut owner2);
        start(&mut h, &mut o, &mut realm2);
        // the player changes the module's data on this realm
        realm2
            .extension_realm
            .data
            .insert(GUID, b"changed on the second realm".to_vec());
        realm2.play(|m| m.progression.money += 1);
        h.tick(&mut realm2, 100).unwrap();
        h.tick(&mut realm2, 101).unwrap();
        deliver_all(&mut h, &mut o, &mut realm2);
    }
    let after = owner2.load_current(id).unwrap();
    assert_eq!(
        after.extensions["mod:fake"].payload.0,
        b"changed on the second realm"
    );
    assert_eq!(
        after.extensions["mod:unknown-module"], original["mod:unknown-module"],
        "the module nobody here has is still exactly what it was"
    );
}
