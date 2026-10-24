use coa_core::realm_registry::client::RegistryClient;
use coa_registry_proto::caps::*;
use coa_registry_proto::sign::encode_public_key;
use coa_registry_proto::*;
use ed25519_dalek::SigningKey;
use uuid::Uuid;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn random_key() -> SigningKey {
    let mut seed = [0u8; 32];
    seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
    seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
    SigningKey::from_bytes(&seed)
}

fn base_url() -> Option<String> {
    std::env::var("REGISTRY_URL").ok().filter(|u| !u.is_empty())
}

fn sample_caps(max_level: u32) -> AdvertisedCapabilities {
    let content = ContentProfile {
        ruleset: Ruleset::Coa,
        character_formats: CharacterFormats { readable: FormatRange { min: 1, max: 2 }, writable: FormatRange { min: 2, max: 2 } },
        online_import_job_formats: vec![2],
        session_protocol: 2,
        collection_protocol: 1,
        features: [Feature::RuntimeSessions, Feature::Wardrobe, Feature::LevelProjection].into_iter().collect(),
        collection_kinds: ["coa:appearance".to_string()].into_iter().collect(),
        extensions: vec![ExtensionSupport { namespace: "coa".into(), module_version: "1".into(), formats: FormatRange { min: 1, max: 1 } }],
        client_catalog: [("Appearances.dbc".to_string(), CatalogEntry { sha256: "a".repeat(64), records: 5 })].into_iter().collect(),
    };
    let content_profile_hash = content.hash();
    AdvertisedCapabilities {
        profile_version: 2,
        core: Some(CoreIdentity { commit: "abc123".into(), branch: "main".into(), date: "2026-10-01".into() }),
        content,
        content_profile_hash,
        progression: Some(Progression { max_player_level: max_level, projection_protocol: 1, projection_policy_version: 1, progression_signature: "b".repeat(64), scaling_enabled: false }),
    }
}

#[test]
#[ignore = "Uses temporary public Registry entries, removed after each assertion"]
fn registry_accepts_all_three_rulesets() {
    let base = base_url().expect("REGISTRY_URL");
    let client = RegistryClient::new(&base).unwrap();
    for ruleset in [Ruleset::Coa, Ruleset::Wildcard, Ruleset::Reborn] {
        let realm = RealmId::new();
        let key = random_key();
        let mut caps = sample_caps(80);
        caps.content.ruleset = ruleset;
        caps.content.features.clear();
        caps.content.collection_kinds.clear();
        caps.content.extensions.clear();
        caps.content.client_catalog.clear();
        caps.content_profile_hash = caps.content.hash();
        caps.progression = None;
        let listing = Listing { display_name: "Candidate Ruleset Gate".into(), description: "Temporary compatibility test".into(), language: "en".into(), region: None, rates: Rates::default(), modules: vec![], account_provisioning: AccountProvisioning { automatic: false, existing_only: true }, manager_version: "0.8.0".into() };
        let req = RegisterRequest { protocol_version: REGISTRY_PROTOCOL_VERSION, realm_id: realm, public_key: encode_public_key(&key.verifying_key()), listing_hash: listing.hash(), listing, capabilities_hash: caps.advert_hash(), capabilities: caps, population: Population { players: 0, bots: 0, capacity: None } };
        let result = client.register(&key, &req, now());
        if result.is_ok() { client.unpublish(&key, &realm, now()+1).unwrap(); }
        assert!(result.is_ok(), "Registry rejected {ruleset:?}: {result:?}");
        println!("Public Registry accepted and removed {ruleset:?} test realm");
    }
}
