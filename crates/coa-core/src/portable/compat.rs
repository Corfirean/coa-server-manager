//! Compatibility (Phase 7): what would happen to a character if it were imported into, updated on, or run on a realm with a given
//! content profile, **decided before anything is written**.
//!
//! Every part of the character gets an explicit [`Outcome`]:
//!
//! * `Compatible`: applied completely;
//! * `Held`: some of it is not applied (the realm's client data does not know it, an extension is in another format) and it stays
//!   in the canonical character for a realm that can take it;
//! * `Unsupported`: the realm lacks the feature altogether; nothing of it is applied and nothing of it is lost;
//! * `Blocking`: the operation itself cannot be done on this realm (another ruleset, a character format the realm cannot read, an
//!   online import the core cannot take, a session it cannot run). It is refused before the first write.
//!
//! The report never changes anything. The operations that write call [`evaluate`] first and refuse on a blocking outcome.

use super::capabilities::{Feature, RealmCapabilities};
use super::collection::IdSet;
use super::extension::{ExtensionDisposition, ExtensionRegistry};
use super::model::PortableCharacter;
use super::realm::knowledge::RealmKnowledge;
use super::versions::ONLINE_IMPORT_JOB_FORMAT_VERSION;

/// What is about to be done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// Import into a stopped realm (the Manager writes the rows itself).
    OfflineImport,
    /// Import into a running realm through the core's import service.
    OnlineImport,
    /// Bring the realm's own character to the canonical state, in place.
    Update,
    /// Run the character in a runtime portable session (the core holds the player until the baseline is taken).
    RuntimeSession,
}

impl std::fmt::Display for Operation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Operation::OfflineImport => "offline import",
            Operation::OnlineImport => "online import",
            Operation::Update => "update",
            Operation::RuntimeSession => "runtime session",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Topic {
    Ruleset,
    CharacterFormat,
    OnlineImport,
    RuntimeSessions,
    ClientData,
    Wardrobe,
    Collection(String),
    Extension(String),
}

impl std::fmt::Display for Topic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Topic::Ruleset => f.write_str("ruleset"),
            Topic::CharacterFormat => f.write_str("character format"),
            Topic::OnlineImport => f.write_str("online import"),
            Topic::RuntimeSessions => f.write_str("runtime sessions"),
            Topic::ClientData => f.write_str("client data"),
            Topic::Wardrobe => f.write_str("wardrobe"),
            Topic::Collection(kind) => write!(f, "collection {kind}"),
            Topic::Extension(ns) => write!(f, "extension {ns}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Compatible(Topic),
    Held { topic: Topic, held: usize, applicable: usize, reason: String },
    Unsupported { topic: Topic, reason: String },
    Blocking { topic: Topic, reason: String },
}

impl Outcome {
    pub fn topic(&self) -> &Topic {
        match self {
            Outcome::Compatible(t) | Outcome::Held { topic: t, .. } | Outcome::Unsupported { topic: t, .. } | Outcome::Blocking { topic: t, .. } => t,
        }
    }
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Outcome::Compatible(t) => write!(f, "{t}: compatible"),
            Outcome::Held { topic, held, applicable, reason } => write!(f, "{topic}: {applicable} applied, {held} held back in the canonical character ({reason})"),
            Outcome::Unsupported { topic, reason } => write!(f, "{topic}: not supported by this realm, nothing applied and nothing lost ({reason})"),
            Outcome::Blocking { topic, reason } => write!(f, "{topic}: BLOCKS the operation ({reason})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Everything is applied.
    Compatible,
    /// The operation can be done; some of the character stays canonical-only.
    Degraded,
    /// The operation cannot be done on this realm.
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityReport {
    pub operation: Operation,
    pub content_profile_hash: String,
    pub outcomes: Vec<Outcome>,
}

impl CompatibilityReport {
    pub fn verdict(&self) -> Verdict {
        if self.outcomes.iter().any(|o| matches!(o, Outcome::Blocking { .. })) {
            Verdict::Incompatible
        } else if self.outcomes.iter().any(|o| !matches!(o, Outcome::Compatible(_))) {
            Verdict::Degraded
        } else {
            Verdict::Compatible
        }
    }

    pub fn blocking(&self) -> Vec<&Outcome> {
        self.outcomes.iter().filter(|o| matches!(o, Outcome::Blocking { .. })).collect()
    }

    pub fn outcome(&self, topic: &Topic) -> Option<&Outcome> {
        self.outcomes.iter().find(|o| o.topic() == topic)
    }

    /// What was not applied completely, one line each (what an operation reports back).
    pub fn held_lines(&self) -> Vec<String> {
        self.outcomes.iter().filter(|o| !matches!(o, Outcome::Compatible(_))).map(|o| o.to_string()).collect()
    }
}

/// Everything the evaluation reads, and nothing it writes.
pub struct Inputs<'a> {
    pub operation: Operation,
    pub model: &'a PortableCharacter,
    pub capabilities: &'a RealmCapabilities,
    /// The ids the realm's client data knows (read from its data directory); `None` when the Manager has not read them.
    pub knowledge: Option<&'a RealmKnowledge>,
    /// The Owner's canonical collections that would be applied to the realm's account alongside.
    pub collections: &'a [(&'a str, &'a IdSet)],
    pub extensions: Option<&'a ExtensionRegistry>,
}

pub fn evaluate(i: &Inputs<'_>) -> CompatibilityReport {
    let content = &i.capabilities.content;
    let mut out = Vec::new();

    if i.model.ruleset == content.ruleset {
        out.push(Outcome::Compatible(Topic::Ruleset));
    } else {
        out.push(Outcome::Blocking { topic: Topic::Ruleset, reason: format!("the character is {} and the realm is {}", i.model.ruleset, content.ruleset) });
    }

    if content.character_formats.readable.contains(i.model.format_version) {
        out.push(Outcome::Compatible(Topic::CharacterFormat));
    } else {
        out.push(Outcome::Blocking { topic: Topic::CharacterFormat, reason: format!("the character is format {} and the realm reads {}", i.model.format_version, content.character_formats.readable) });
    }

    if i.operation == Operation::OnlineImport {
        if content.online_import_job_formats.contains(&ONLINE_IMPORT_JOB_FORMAT_VERSION) {
            out.push(Outcome::Compatible(Topic::OnlineImport));
        } else if content.online_import_job_formats.is_empty() {
            out.push(Outcome::Blocking { topic: Topic::OnlineImport, reason: "the realm's core did not report an import job format (it was not asked, or it is too old to answer)".into() });
        } else {
            out.push(Outcome::Blocking { topic: Topic::OnlineImport, reason: format!("the core reads job formats {:?}; this Manager writes {ONLINE_IMPORT_JOB_FORMAT_VERSION}", content.online_import_job_formats) });
        }
    }

    if i.operation == Operation::RuntimeSession {
        if content.supports(Feature::RuntimeSessions) {
            out.push(Outcome::Compatible(Topic::RuntimeSessions));
        } else {
            out.push(Outcome::Blocking { topic: Topic::RuntimeSessions, reason: "the realm has no portable session support (the core's session table or switch is missing)".into() });
        }
    }

    if let Some(k) = i.knowledge {
        if let Some(why) = catalog_mismatch(k, i.capabilities) {
            out.push(Outcome::Blocking { topic: Topic::ClientData, reason: why });
        }
    }

    out.push(wardrobe_outcome(i));
    for (kind, set) in i.collections {
        out.push(collection_outcome(i, kind, set));
    }
    if !i.model.extensions.is_empty() {
        out.extend(extension_outcomes(i));
    }

    CompatibilityReport { operation: i.operation, content_profile_hash: i.capabilities.content_profile_hash.clone(), outcomes: out }
}

/// The client tables the Manager read must be the ones the realm's profile describes.
fn catalog_mismatch(knowledge: &RealmKnowledge, caps: &RealmCapabilities) -> Option<String> {
    for (name, mine) in knowledge.catalog() {
        match caps.content.client_catalog.get(name) {
            Some(theirs) if theirs == mine => {}
            Some(_) => return Some(format!("{name} as the Manager reads it is not the {name} the realm loaded")),
            None => return Some(format!("the Manager read {name}, which the realm's profile does not list")),
        }
    }
    None
}

fn wardrobe_outcome(i: &Inputs<'_>) -> Outcome {
    let ids = i.model.wardrobe.ids();
    if ids.is_empty() {
        return Outcome::Compatible(Topic::Wardrobe);
    }
    if !i.capabilities.content.supports(Feature::Wardrobe) {
        return Outcome::Unsupported { topic: Topic::Wardrobe, reason: "the realm has no appearance tables or no Appearances.dbc".into() };
    }
    let Some(knowledge) = i.knowledge else {
        return Outcome::Held { topic: Topic::Wardrobe, held: ids.len(), applicable: 0, reason: "the Manager has not read the realm's client data".into() };
    };
    let applicable = ids.iter().filter(|id| knowledge.knows_appearance(**id)).count();
    if applicable == ids.len() {
        Outcome::Compatible(Topic::Wardrobe)
    } else {
        Outcome::Held { topic: Topic::Wardrobe, held: ids.len() - applicable, applicable, reason: "the realm's client data does not know these appearances".into() }
    }
}

fn collection_outcome(i: &Inputs<'_>, kind: &str, set: &IdSet) -> Outcome {
    let topic = Topic::Collection(kind.to_string());
    let content = &i.capabilities.content;
    if !content.supports(Feature::Collections) || !content.collection_kinds.contains(kind) {
        return Outcome::Unsupported { topic, reason: "the realm does not carry this collection".into() };
    }
    let Some(knowledge) = i.knowledge else {
        return Outcome::Held { topic, held: set.len(), applicable: 0, reason: "the Manager has not read the realm's client data".into() };
    };
    let applicable = set
        .ids()
        .iter()
        .filter(|id| match kind {
            "coa:appearance" => knowledge.knows_appearance(**id),
            _ => knowledge.knows_vanity(**id) && !super::realm::knowledge::is_bank_vanity_item(**id),
        })
        .count();
    if applicable == set.len() {
        Outcome::Compatible(topic)
    } else {
        Outcome::Held { topic, held: set.len() - applicable, applicable, reason: "the realm's client data does not know these ids".into() }
    }
}

fn extension_outcomes(i: &Inputs<'_>) -> Vec<Outcome> {
    let content = &i.capabilities.content;
    let empty = ExtensionRegistry::new();
    let registry = i.extensions.unwrap_or(&empty);
    registry
        .evaluate(&i.model.extensions, content)
        .into_iter()
        .map(|o| {
            let topic = Topic::Extension(o.namespace.clone());
            match o.disposition {
                ExtensionDisposition::Applied | ExtensionDisposition::AlreadyApplied => Outcome::Compatible(topic),
                ExtensionDisposition::CanonicalOnly => Outcome::Compatible(topic),
                ExtensionDisposition::Deferred(why) => Outcome::Held { topic, held: 1, applicable: 0, reason: why.to_string() },
                ExtensionDisposition::Failed(why) => Outcome::Held { topic, held: 1, applicable: 0, reason: why },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::capabilities::*;
    use crate::portable::extension::fake::{payload, FakeAdapter};
    use crate::portable::fixtures::geared_level_eighty;
    use crate::portable::model::Ruleset;

    fn profile(features: &[Feature], jobs: &[u32]) -> RealmCapabilities {
        let mut catalog = ClientCatalog::new();
        catalog.insert("Appearances.dbc".into(), CatalogEntry { sha256: "ab".repeat(32), records: 3 });
        RealmCapabilities::new(
            None,
            ContentProfile {
                ruleset: Ruleset::Coa,
                character_formats: ContentProfile::manager_formats(),
                online_import_job_formats: jobs.to_vec(),
                session_protocol: 1,
                collection_protocol: 1,
                features: features.iter().copied().collect(),
                collection_kinds: ["coa:appearance".to_string(), "coa:vanity".to_string()].into_iter().collect(),
                extensions: vec![],
                client_catalog: catalog,
            },
        )
        .unwrap()
    }

    fn all() -> Vec<Feature> {
        vec![Feature::RuntimeSessions, Feature::Wardrobe, Feature::Collections]
    }

    fn knows(a: &[u32]) -> RealmKnowledge {
        RealmKnowledge::new(IdSet::from_ids(a.iter().copied()).unwrap(), IdSet::from_ids([50_001]).unwrap())
    }

    fn run(op: Operation, model: &PortableCharacter, caps: &RealmCapabilities, k: Option<&RealmKnowledge>, collections: &[(&str, &IdSet)], reg: Option<&ExtensionRegistry>) -> CompatibilityReport {
        evaluate(&Inputs { operation: op, model, capabilities: caps, knowledge: k, collections, extensions: reg })
    }

    #[test]
    fn a_plain_character_on_a_matching_realm_is_compatible() {
        let r = run(Operation::OnlineImport, &geared_level_eighty(), &profile(&all(), &[2]), None, &[], None);
        assert_eq!(r.verdict(), Verdict::Compatible, "{:#?}", r.outcomes);
        assert!(r.held_lines().is_empty());
    }

    #[test]
    fn what_cannot_be_done_on_a_realm_is_refused_with_a_reason_before_anything_is_written() {
        let model = geared_level_eighty();
        // a core that reads another job format, one that was not asked, a realm without sessions, another ruleset
        let r = run(Operation::OnlineImport, &model, &profile(&all(), &[3]), None, &[], None);
        assert_eq!(r.verdict(), Verdict::Incompatible);
        assert!(matches!(r.outcome(&Topic::OnlineImport), Some(Outcome::Blocking { reason, .. }) if reason.contains("[3]")));
        let r = run(Operation::OnlineImport, &model, &profile(&all(), &[]), None, &[], None);
        assert!(matches!(r.outcome(&Topic::OnlineImport), Some(Outcome::Blocking { reason, .. }) if reason.contains("did not report")));
        // the same character through the offline importer does not need the core's job format
        assert_eq!(run(Operation::OfflineImport, &model, &profile(&all(), &[]), None, &[], None).verdict(), Verdict::Compatible);
        let r = run(Operation::RuntimeSession, &model, &profile(&[Feature::Wardrobe], &[2]), None, &[], None);
        assert!(matches!(r.outcome(&Topic::RuntimeSessions), Some(Outcome::Blocking { .. })));
        let mut wildcard = model.clone();
        wildcard.ruleset = Ruleset::Wildcard;
        assert!(matches!(run(Operation::Update, &wildcard, &profile(&all(), &[2]), None, &[], None).outcome(&Topic::Ruleset), Some(Outcome::Blocking { .. })));
        // a realm whose Manager side only reads format 1 cannot take a format 2 character
        let mut old = profile(&all(), &[2]).content;
        old.character_formats.readable = FormatRange::single(1);
        let old = RealmCapabilities::new(None, old).unwrap();
        assert!(matches!(run(Operation::Update, &model, &old, None, &[], None).outcome(&Topic::CharacterFormat), Some(Outcome::Blocking { .. })));
    }

    #[test]
    fn a_wardrobe_the_realm_cannot_fully_show_is_held_back_by_count_and_one_it_cannot_show_at_all_is_unsupported() {
        let mut model = geared_level_eighty();
        model.wardrobe.active.insert(1, 100);
        model.wardrobe.active.insert(2, 200);
        model.wardrobe.outfits.insert("Set".into(), vec![100, 300, 0]);
        let caps = profile(&all(), &[2]);
        let r = run(Operation::Update, &model, &caps, Some(&knows(&[100, 200])), &[], None);
        assert_eq!(r.verdict(), Verdict::Degraded);
        assert_eq!(r.outcome(&Topic::Wardrobe), Some(&Outcome::Held { topic: Topic::Wardrobe, held: 1, applicable: 2, reason: "the realm's client data does not know these appearances".into() }));
        assert_eq!(run(Operation::Update, &model, &caps, Some(&knows(&[100, 200, 300])), &[], None).verdict(), Verdict::Compatible);
        // the Manager has not read the data: everything is held, nothing is guessed
        assert!(matches!(run(Operation::Update, &model, &caps, None, &[], None).outcome(&Topic::Wardrobe), Some(Outcome::Held { applicable: 0, held: 3, .. })));
        // the realm has no wardrobe feature: nothing is applied, nothing is lost
        let r = run(Operation::Update, &model, &profile(&[Feature::RuntimeSessions], &[2]), Some(&knows(&[100])), &[], None);
        assert!(matches!(r.outcome(&Topic::Wardrobe), Some(Outcome::Unsupported { .. })));
        assert_eq!(r.verdict(), Verdict::Degraded, "an unsupported feature degrades; it does not block");
        // a character that wears nothing has nothing to hold
        assert_eq!(run(Operation::Update, &geared_level_eighty(), &profile(&[], &[2]), None, &[], None).verdict(), Verdict::Compatible);
    }

    #[test]
    fn collections_are_counted_per_kind_and_the_bank_items_are_never_applicable() {
        let appearances = IdSet::from_ids([100, 200, 300]).unwrap();
        let vanity = IdSet::from_ids([50_001, 50_002, 110_000]).unwrap();
        let caps = profile(&all(), &[2]);
        let k = RealmKnowledge::new(IdSet::from_ids([100, 200]).unwrap(), IdSet::from_ids([50_001, 110_000]).unwrap());
        let r = run(Operation::Update, &geared_level_eighty(), &caps, Some(&k), &[("coa:appearance", &appearances), ("coa:vanity", &vanity)], None);
        assert!(matches!(r.outcome(&Topic::Collection("coa:appearance".into())), Some(Outcome::Held { held: 1, applicable: 2, .. })));
        assert!(matches!(r.outcome(&Topic::Collection("coa:vanity".into())), Some(Outcome::Held { held: 2, applicable: 1, .. })), "50002 is unknown and the bank item is not carried");
        let r = run(Operation::Update, &geared_level_eighty(), &profile(&[Feature::Wardrobe], &[2]), Some(&k), &[("coa:appearance", &appearances)], None);
        assert!(matches!(r.outcome(&Topic::Collection("coa:appearance".into())), Some(Outcome::Unsupported { .. })));
    }

    #[test]
    fn extensions_are_applicable_deferred_or_canonical_only_and_never_block() {
        let mut model = geared_level_eighty();
        model.extensions.insert("mod:fake".into(), payload(2, b"x"));
        model.extensions.insert("mod:other".into(), payload(1, b"y"));
        model.extensions.insert("coa:unlisted-settings".into(), payload(1, b"{}"));
        let mut reg = ExtensionRegistry::new();
        reg.register(FakeAdapter::new(2, 3)).unwrap();
        let mut caps = profile(&all(), &[2]).content;
        caps.extensions = vec![ExtensionSupport { namespace: "mod:fake".into(), module_version: "1.0.0".into(), formats: FormatRange::new(2, 3) }];
        let caps = RealmCapabilities::new(None, caps).unwrap();
        let r = run(Operation::Update, &model, &caps, None, &[], Some(&reg));
        assert_eq!(r.outcome(&Topic::Extension("mod:fake".into())), Some(&Outcome::Compatible(Topic::Extension("mod:fake".into()))));
        assert!(matches!(r.outcome(&Topic::Extension("mod:other".into())), Some(Outcome::Held { reason, .. }) if reason.contains("does not have this module")));
        assert_eq!(r.verdict(), Verdict::Degraded);
        assert!(r.blocking().is_empty());
        // the same payload in a format the realm does not read
        model.extensions.insert("mod:fake".into(), payload(4, b"x"));
        let r = run(Operation::Update, &model, &caps, None, &[], Some(&reg));
        assert!(matches!(r.outcome(&Topic::Extension("mod:fake".into())), Some(Outcome::Held { reason, .. }) if reason.contains("newer")));
    }

    #[test]
    fn client_data_that_is_not_what_the_realm_loaded_blocks() {
        let caps = profile(&all(), &[2]);
        let mut k = knows(&[100]);
        k.set_catalog_for_test("Appearances.dbc", &"ab".repeat(32), 3);
        assert_eq!(run(Operation::Update, &geared_level_eighty(), &caps, Some(&k), &[], None).verdict(), Verdict::Compatible);
        k.set_catalog_for_test("Appearances.dbc", &"cd".repeat(32), 3);
        let r = run(Operation::Update, &geared_level_eighty(), &caps, Some(&k), &[], None);
        assert!(matches!(r.outcome(&Topic::ClientData), Some(Outcome::Blocking { .. })));
        let mut k = knows(&[100]);
        k.set_catalog_for_test("VanityCollection.dbc", &"ab".repeat(32), 3);
        assert!(matches!(run(Operation::Update, &geared_level_eighty(), &caps, Some(&k), &[], None).outcome(&Topic::ClientData), Some(Outcome::Blocking { reason, .. }) if reason.contains("does not list")));
    }
}
