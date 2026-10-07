//! The realm side of Phase 7: building a realm's capabilities, gating an operation on them, and what follows a successful write
//! (the profile hash the character was synchronised under, the extensions an adapter can apply).

use std::path::Path;

use crate::db::Db;
use crate::ra::Ra;

use super::super::capabilities::*;
use super::super::compat::{self, CompatibilityReport, Inputs, Operation, Verdict};
use super::super::error::{PortableError, Result};
use super::super::extension::{DbExtensionRealm, ExtensionDisposition, ExtensionOutcome, ExtensionRegistry};
use super::super::ids::CharacterId;
use super::super::model::PortableCharacter;
use super::super::session::protocol::{COLLECTION_KINDS, PROTOCOL_VERSION};
use super::super::store::Store;
use super::import::ImportOptions;
use super::script::SchemaProbe;
use super::wardrobe;
use super::{probe, ruleset_of};

/// Ask the realm's core for its capability report (RA `portable capabilities`).
pub fn core_report(ra: &mut Ra) -> Result<CoreReport> {
    ra.portable_capabilities().map_err(|e| PortableError::RealmRead(e.to_string()))
}

/// Build a realm's capabilities from what can be seen of it.
///
/// * `probe`: the realm's characters schema (which module tables exist);
/// * `data_dir`: the realm's `Data` directory, whose client tables are hashed (the catalog) and must match what the core reports it
///   loaded;
/// * `core`: the core's own report, when it could be asked. Without it the identity and the online-import job formats are unknown and
///   the runtime-session feature cannot be claimed;
/// * `registry`: the extension adapters this Manager has, listed when their module is on the realm.
pub fn assemble(db: &Db, probe: &SchemaProbe, data_dir: Option<&Path>, core: Option<&CoreReport>, registry: &ExtensionRegistry) -> Result<RealmCapabilities> {
    let mut catalog = ClientCatalog::new();
    if let Some(dir) = data_dir {
        for name in CATALOG_TABLES {
            if let Some(entry) = super::knowledge::table_entry(&dir.join("dbc").join(name)) {
                catalog.insert(name.to_string(), entry);
            }
        }
    }
    if let Some(report) = core {
        // what the core loaded must be what the Manager sees in the data directory, table by table
        for (name, theirs) in report.catalog() {
            if let Some(mine) = catalog.get(&name) {
                if *mine != theirs {
                    return Err(PortableError::SchemaMismatch(format!("{name}: the realm's core loaded another file than the one in {}", data_dir.map(|d| d.display().to_string()).unwrap_or_default())));
                }
            } else if data_dir.is_none() {
                catalog.insert(name, theirs);
            }
        }
    }

    let mut features = std::collections::BTreeSet::new();
    let have_catalog = |name: &str| catalog.contains_key(name);
    if wardrobe::tables_present(probe) && have_catalog("Appearances.dbc") {
        features.insert(Feature::Wardrobe);
    }
    if probe.has("account_appearance_collection") && probe.has("account_vanity_collection") && have_catalog("Appearances.dbc") && have_catalog("VanityCollection.dbc") {
        features.insert(Feature::Collections);
    }
    if probe.has("coa_portable_session") && core.is_some_and(|c| c.has_feature("runtime_sessions")) {
        features.insert(Feature::RuntimeSessions);
    }
    let progression = core.and_then(|c| c.progression.clone());
    if progression.is_some() && core.is_some_and(|c| c.has_feature("level_projection")) {
        features.insert(Feature::LevelProjection);
    }

    let mut extension_realm = NoRealm { probe };
    let extensions = registry.supported_on(&mut extension_realm);
    let content = ContentProfile {
        ruleset: ruleset_of(db),
        character_formats: ContentProfile::manager_formats(),
        online_import_job_formats: core.map(|c| c.portable.job_formats.clone()).unwrap_or_default(),
        session_protocol: PROTOCOL_VERSION,
        collection_protocol: PROTOCOL_VERSION,
        features,
        collection_kinds: if probe.has("account_appearance_collection") && probe.has("account_vanity_collection") { COLLECTION_KINDS.iter().map(|k| k.to_string()).collect() } else { Default::default() },
        extensions,
        client_catalog: catalog,
    };
    RealmCapabilities::build(core.map(|c| c.core.clone()), content, progression)
}

/// Probe a running or stopped realm: the schema from the database, the core from RA when a console is given.
pub fn probe_capabilities(db: &Db, data_dir: Option<&Path>, ra: Option<&mut Ra>, registry: &ExtensionRegistry) -> Result<RealmCapabilities> {
    let schema = probe(db)?;
    let report = match ra {
        Some(ra) => Some(core_report(ra)?),
        None => None,
    };
    assemble(db, &schema, data_dir, report.as_ref(), registry)
}

/// A profile assembled without the realm's core (a stopped realm) knows nothing of its progression. The progression the realm's core last
/// reported live (remembered in the store) is the best there is: the profile gets it, and the core still refuses a character prepared for
/// another progression at login, so a realm whose rules changed since is caught, not trusted.
pub fn with_remembered_progression(offline: RealmCapabilities, remembered: Option<&RealmCapabilities>) -> Result<RealmCapabilities> {
    let Some(progression) = remembered.and_then(|r| r.progression.clone()) else { return Ok(offline) };
    if offline.progression.is_some() {
        return Ok(offline);
    }
    let mut content = offline.content.clone();
    if remembered.is_some_and(|r| r.content.supports(Feature::LevelProjection)) {
        content.features.insert(Feature::LevelProjection);
    }
    RealmCapabilities::build(offline.core.clone(), content, Some(progression))
}

/// The table-level view an adapter needs to say whether its module is present, before any character is involved.
struct NoRealm<'a> {
    probe: &'a SchemaProbe,
}

impl super::super::extension::ExtensionRealm for NoRealm<'_> {
    fn local_guid(&self) -> u32 {
        0
    }
    fn has_table(&self, table: &str) -> bool {
        self.probe.has(table)
    }
    fn query(&mut self, _: &str) -> Result<String> {
        Err(PortableError::Invalid("no character is selected: this view only tells which tables exist".into()))
    }
    fn execute(&mut self, _: &str) -> Result<()> {
        Err(PortableError::Invalid("no character is selected: this view only tells which tables exist".into()))
    }
}

/// Evaluate `model` for the operations about to be done, and refuse before anything is written when one of them cannot be done.
/// Without capabilities in the options nothing is evaluated (the lower layers' own tests, and tools that do not know the realm).
pub fn gate(opts: &ImportOptions, model: &PortableCharacter, operations: &[Operation]) -> Result<Option<CompatibilityReport>> {
    let Some(caps) = opts.capabilities.as_deref() else { return Ok(None) };
    let mut merged: Option<CompatibilityReport> = None;
    for op in operations {
        let decider = opts.projection.is_some() || operations.contains(&Operation::OnlineImport);
        let report = compat::evaluate(&Inputs { operation: *op, model, capabilities: caps, knowledge: opts.knowledge.as_deref(), collections: &[], extensions: opts.extensions.as_deref(), projection_decider: decider });
        merged = Some(match merged {
            None => report,
            Some(mut m) => {
                for o in report.outcomes {
                    if !m.outcomes.iter().any(|e| e.topic() == o.topic()) {
                        m.outcomes.push(o);
                    }
                }
                m
            }
        });
    }
    let report = merged.ok_or_else(|| PortableError::Invalid("an operation is needed to evaluate".into()))?;
    if report.verdict() == Verdict::Incompatible {
        return Err(PortableError::Incompatible { operation: report.operation.to_string(), reasons: report.blocking().iter().map(|o| o.to_string()).collect() });
    }
    Ok(Some(report))
}

/// After a character was written to a realm: remember the profile it was synchronised under, apply the extensions an adapter can
/// take, and say in words everything that was not applied completely.
pub fn after_write(db: Option<&Db>, store: &mut Store, id: CharacterId, server_id: &str, opts: &ImportOptions, guid: u32, model: &PortableCharacter, report: Option<&CompatibilityReport>) -> Result<Vec<String>> {
    let Some(caps) = opts.capabilities.as_deref() else { return Ok(vec![]) };
    let mut lines: Vec<String> = report.map(|r| r.held_lines().into_iter().filter(|l| !l.starts_with("extension ")).collect()).unwrap_or_default();
    let outcomes = match db {
        Some(db) => apply_extensions(db, store, id, server_id, opts, guid, model)?,
        None if model.extensions.keys().any(|k| super::super::extension::is_module_namespace(k)) && opts.extensions.is_some() => {
            lines.push("module extensions were not applied: the online import was not given the realm's database connection (they stay canonical)".to_string());
            vec![]
        }
        None => vec![],
    };
    lines.extend(outcomes.iter().filter(|o| !matches!(o.disposition, ExtensionDisposition::Applied | ExtensionDisposition::AlreadyApplied | ExtensionDisposition::CanonicalOnly)).map(|o| o.to_string()));
    store.set_mapping_profile(id, server_id, &caps.content_profile_hash)?;
    Ok(lines)
}

/// Apply the canonical extensions to the character on the realm, once per payload, and record what was applied.
pub fn apply_extensions(db: &Db, store: &mut Store, id: CharacterId, server_id: &str, opts: &ImportOptions, guid: u32, model: &PortableCharacter) -> Result<Vec<ExtensionOutcome>> {
    let (Some(caps), Some(registry)) = (opts.capabilities.as_deref(), opts.extensions.as_deref()) else { return Ok(vec![]) };
    if model.extensions.is_empty() {
        return Ok(vec![]);
    }
    let schema = probe(db)?;
    let applied = store.extension_state(id, server_id)?;
    let mut realm = DbExtensionRealm::new(db, guid, &schema);
    let outcomes = registry.apply_all(&mut realm, &model.extensions, &caps.content, &applied);
    for o in &outcomes {
        if o.disposition == ExtensionDisposition::Applied {
            if let Some(ext) = model.extensions.get(&o.namespace) {
                store.set_extension_applied(id, server_id, &o.namespace, &ext.content_hash)?;
            }
        }
    }
    Ok(outcomes)
}

/// The extensions of the character that the realm's adapters can read, to be merged as the realm's side (B0, B1). A problem of an
/// adapter is returned as text and contributes nothing.
pub fn export_extensions(db: &Db, schema: &SchemaProbe, guid: u32, registry: &ExtensionRegistry, model: &mut PortableCharacter) -> Vec<String> {
    let mut realm = DbExtensionRealm::new(db, guid, schema);
    let (exported, problems) = registry.export_all(&mut realm);
    model.extensions.retain(|k, _| !super::super::extension::is_module_namespace(k));
    model.extensions.extend(exported);
    problems
}
