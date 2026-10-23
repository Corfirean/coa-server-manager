//! Live tests of Phase 7 against the two real disposable MySQL servers of `live_import.rs`: a realm's profile is assembled from what
//! can be seen of it, an operation is evaluated before it writes, and a change of the profile brings back what was held back.
//! `#[ignore]`d; see `live_import.rs` for how to run. `COA_PORTABLE_LIVE_DATA` is a `Data` directory with `dbc/Appearances.dbc` and
//! `dbc/VanityCollection.dbc` (default `C:/games/coa-fixture/Data`).

use std::path::PathBuf;
use std::sync::Arc;

use crate::portable::capabilities::*;
use crate::portable::collection::IdSet;
use crate::portable::extension::ExtensionRegistry;
use crate::portable::model::Ruleset;

use super::knowledge::RealmKnowledge;
use super::live_import::{
    counts, fresh_store, make, opts_without_knowledge, realms, reset_b, ACCOUNT, B,
};
use super::*;

fn data_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(
        std::env::var("COA_PORTABLE_LIVE_DATA")
            .unwrap_or_else(|_| "C:/games/coa-fixture/Data".into()),
    );
    dir.join("dbc/Appearances.dbc").exists().then_some(dir)
}

fn knows(appearances: &[u32]) -> Arc<RealmKnowledge> {
    Arc::new(RealmKnowledge::new(
        IdSet::from_ids(appearances.iter().copied()).unwrap(),
        IdSet::new(),
    ))
}

fn caps(
    marker: u8,
    features: &[Feature],
    jobs: &[u32],
    ruleset: Ruleset,
    readable: FormatRange,
) -> Arc<RealmCapabilities> {
    let mut catalog = ClientCatalog::new();
    catalog.insert(
        "Appearances.dbc".into(),
        CatalogEntry {
            sha256: format!("{marker:02x}").repeat(32),
            records: 9,
        },
    );
    Arc::new(
        RealmCapabilities::build(
            None,
            ContentProfile {
                ruleset,
                character_formats: CharacterFormats {
                    readable,
                    writable: FormatRange::single(2),
                },
                online_import_job_formats: jobs.to_vec(),
                session_protocol: 2,
                collection_protocol: 2,
                features: features.iter().copied().collect(),
                collection_kinds: ["coa:appearance".to_string(), "coa:vanity".to_string()]
                    .into_iter()
                    .collect(),
                extensions: vec![],
                client_catalog: catalog,
            },
            Some(crate::portable::capabilities::Progression {
                max_player_level: 80,
                projection_protocol: 1,
                projection_policy_version: 1,
                progression_signature: "ee".repeat(32),
                scaling_enabled: true,
            }),
        )
        .unwrap(),
    )
}

fn all() -> Vec<Feature> {
    vec![Feature::Wardrobe, Feature::Collections]
}

fn selected(db: &crate::db::Db, guid: u32) -> Vec<String> {
    db.query(&format!("SELECT CONCAT(category_id, '=', appearance_id) FROM acore_characters.character_appearance WHERE guid = {guid} ORDER BY category_id")).unwrap().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

#[test]
#[ignore]
fn a_realms_profile_is_assembled_from_its_schema_and_client_data_and_is_stable() {
    let Some(r) = realms() else { return };
    let Some(dir) = data_dir() else { return };
    let registry = ExtensionRegistry::new();
    let schema = probe(&r.b).unwrap();
    let a = super::profile::assemble(&r.b, &schema, Some(&dir), None, &registry).unwrap();
    let again = super::profile::assemble(&r.b, &schema, Some(&dir), None, &registry).unwrap();
    assert_eq!(a, again);
    let c = &a.content;
    assert_eq!(c.ruleset, Ruleset::Coa);
    assert!(c.supports(Feature::Wardrobe) && c.supports(Feature::Collections));
    assert!(
        !c.supports(Feature::RuntimeSessions),
        "a session needs the core's own report; the Manager does not guess it"
    );
    assert!(
        c.online_import_job_formats.is_empty(),
        "the job formats are the core's to say"
    );
    assert_eq!(c.collection_kinds.len(), 2);
    assert!(a.core.is_none());
    for table in [
        "Appearances.dbc",
        "VanityCollection.dbc",
        "ItemAppearances.dbc",
    ] {
        let e = &c.client_catalog[table];
        assert_eq!(e.sha256.len(), 64);
        assert!(e.records > 1000, "{table}: {e:?}");
    }
    // the ids the Manager reads are the tables the profile hashes: the same files
    let knowledge = RealmKnowledge::from_data_dir(&dir).unwrap();
    for (name, entry) in knowledge.catalog() {
        assert_eq!(&c.client_catalog[name], entry, "{name}");
    }
    assert!(
        a.to_json().unwrap().len() < 4096,
        "no id list: a profile is a few hundred bytes of hashes"
    );
    // another data directory is another content profile
    let other = tempfile::tempdir().unwrap();
    std::fs::create_dir(other.path().join("dbc")).unwrap();
    std::fs::copy(
        dir.join("dbc/Appearances.dbc"),
        other.path().join("dbc/Appearances.dbc"),
    )
    .unwrap();
    let narrower =
        super::profile::assemble(&r.b, &schema, Some(other.path()), None, &registry).unwrap();
    assert!(
        !narrower.content.supports(Feature::Collections),
        "without VanityCollection.dbc the collections cannot be carried"
    );
    assert_ne!(narrower.content_profile_hash, a.content_profile_hash);
}

#[test]
#[ignore]
fn an_operation_a_realm_cannot_take_is_refused_before_one_row_is_written() {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let before = counts(&r.b);

    for (why, caps) in [
        (
            "another ruleset",
            caps(1, &all(), &[2], Ruleset::Wildcard, FormatRange::new(1, 2)),
        ),
        (
            "a realm that only reads the old character format",
            caps(1, &all(), &[2], Ruleset::Coa, FormatRange::single(1)),
        ),
    ] {
        let opts = ImportOptions {
            capabilities: Some(caps),
            ..opts_without_knowledge()
        };
        let error = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts).expect_err(why);
        assert!(
            matches!(error, PortableError::Incompatible { .. }),
            "{why}: {error}"
        );
        assert!(error.to_string().contains("nothing was written"), "{error}");
    }
    assert_eq!(counts(&r.b), before, "not one row of the realm changed");
    assert!(
        store
            .server_mappings(id)
            .unwrap()
            .iter()
            .all(|m| m.server_id != B),
        "and no mapping was made"
    );
    assert!(
        store.open_imports(B).unwrap().is_empty(),
        "nor a journal entry"
    );

    // a session on a realm without the session feature
    let opts = ImportOptions {
        capabilities: Some(caps(1, &all(), &[2], Ruleset::Coa, FormatRange::new(1, 2))),
        ..opts_without_knowledge()
    };
    let session = crate::portable::ids::SessionId::new();
    assert!(matches!(
        import_character_in_session(&r.b, &mut store, id, B, ACCOUNT, &opts, Some(session)),
        Err(PortableError::Incompatible { .. })
    ));
    assert_eq!(counts(&r.b), before);

    // a realm that can take it, but not all of it: the import goes ahead and says what stays canonical
    let opts = ImportOptions {
        capabilities: Some(caps(1, &all(), &[2], Ruleset::Coa, FormatRange::new(1, 2))),
        knowledge: Some(knows(&[1101, 1104])),
        ..opts_without_knowledge()
    };
    let outcome = import_character(&r.b, &mut store, id, B, ACCOUNT, &opts).unwrap();
    assert!(
        outcome
            .not_applied
            .iter()
            .any(|l| l.contains("wardrobe") && l.contains("1 held back")),
        "{:?}",
        outcome.not_applied
    );
    assert_eq!(selected(&r.b, outcome.local_guid), ["1=1101", "4=1104"]);
    assert_eq!(
        store.mapping_profile(id, B).unwrap().as_deref(),
        Some(
            opts.capabilities
                .as_ref()
                .unwrap()
                .content_profile_hash
                .as_str()
        ),
        "the character remembers the profile it was synchronised under"
    );
}

#[test]
#[ignore]
fn a_changed_profile_brings_back_what_was_held_back_at_the_same_revision_and_leaves_the_realms_own_choices_alone(
) {
    let Some(r) = realms() else { return };
    reset_b(&r.b);
    let (mut store, profile) = fresh_store();
    let id = make(&r, &mut store, profile, 1002);
    let narrow = ImportOptions {
        capabilities: Some(caps(1, &all(), &[2], Ruleset::Coa, FormatRange::new(1, 2))),
        knowledge: Some(knows(&[1101, 1104])),
        ..opts_without_knowledge()
    };
    let g = import_character(&r.b, &mut store, id, B, ACCOUNT, &narrow)
        .unwrap()
        .local_guid;
    assert_eq!(selected(&r.b, g), ["1=1101", "4=1104"], "1156 is held back");
    let revision = store.character(id).unwrap().revision;

    // the same profile: nothing is looked at
    let same = reevaluate_realm_character(&r.b, &mut store, id, B, &narrow).unwrap();
    assert!(same.profile_unchanged && same.update.is_none());

    // the player of that realm chooses something of its own in a category the canonical character has nothing in
    r.b.query(&format!("INSERT INTO acore_characters.character_appearance (guid, category_id, appearance_id) VALUES ({g}, 9, 1104)")).unwrap();
    // and the realm's client data is replaced by a newer one that knows 1156
    let wide = ImportOptions {
        capabilities: Some(caps(2, &all(), &[2], Ruleset::Coa, FormatRange::new(1, 2))),
        knowledge: Some(knows(&[1101, 1104, 1156])),
        ..opts_without_knowledge()
    };
    let outcome = reevaluate_realm_character(&r.b, &mut store, id, B, &wide).unwrap();
    assert!(!outcome.profile_unchanged);
    assert!(
        outcome.update.as_ref().is_some_and(|u| u.updated),
        "{outcome:?}"
    );
    assert_eq!(
        selected(&r.b, g),
        ["1=1101", "4=1104", "9=1104", "56=1156"],
        "the held-back selection is written, the realm's own stays"
    );
    assert_eq!(
        store.character(id).unwrap().revision,
        revision,
        "the canonical revision did not move"
    );
    assert_eq!(
        store.mapping_profile(id, B).unwrap().as_deref(),
        Some(
            wide.capabilities
                .as_ref()
                .unwrap()
                .content_profile_hash
                .as_str()
        )
    );
    assert!(store.open_imports(B).unwrap().is_empty());

    // and now it is quiet again
    assert!(
        reevaluate_realm_character(&r.b, &mut store, id, B, &wide)
            .unwrap()
            .profile_unchanged
    );
    // a profile that changed but has nothing new to show writes nothing
    let third = ImportOptions {
        capabilities: Some(caps(3, &all(), &[2], Ruleset::Coa, FormatRange::new(1, 2))),
        knowledge: Some(knows(&[1101, 1104, 1156])),
        ..opts_without_knowledge()
    };
    let before = counts(&r.b);
    let outcome = reevaluate_realm_character(&r.b, &mut store, id, B, &third).unwrap();
    assert!(!outcome.profile_unchanged && outcome.update.is_none());
    assert_eq!(counts(&r.b), before);
    assert_eq!(
        store.mapping_profile(id, B).unwrap().as_deref(),
        Some(
            third
                .capabilities
                .as_ref()
                .unwrap()
                .content_profile_hash
                .as_str()
        )
    );
}
