//! The production importer, Manager side: into a **running** realm, by the core's `PortableImportService`.
//!
//! ```text
//! begin_import (journal "prepared")  ->  <JobDir>/<job_id>.job  ->  RA `portable import <job_id>`  ->  <job_id>.result
//!                                                                    (the core imports in one transaction and answers)
//!   -> finish_import (mappings + journal "committed")
//! ```
//!
//! Only the job id travels through RA. The job file is written to a fixed directory (the realm's `PortableImport.JobDir`)
//! under a temporary name and renamed, and holds exactly what the realm needs: the character without extensions and without
//! settings the policy does not carry.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ra::Ra;

use super::super::error::{PortableError, Result};
use super::super::ids::{CharacterId, ImportId, SessionId};
use super::super::model::PortableCharacter;
use super::super::snapshot;
use super::super::store::{ImportAllocation, Store};
use super::import::{planned_items, planned_pets, ImportOptions, ImportOutcome};
use super::policy::{classify_setting, Disposition};

/// The header line of a job file.
#[derive(Debug, Serialize)]
struct JobHeader {
    /// The job format: the core refuses a job whose format it does not read before it parses anything else.
    job_format: u32,
    job_id: ImportId,
    nonce: [u32; 4],
    account: u32,
    revision: u64,
    max_characters_per_account: u32,
    snapshot_sha256: String,
    character_id: CharacterId,
    session: Option<SessionHeader>,
    /// The progression the character was prepared for: the core refuses a job prepared for another one, and pins the character to it.
    projection: Option<ProjectionHeader>,
}

#[derive(Debug, Serialize)]
struct ProjectionHeader {
    /// The character was projected down to the cap (and the core checks that nothing above the cap is left in it).
    active: bool,
    level: u32,
    policy_version: u32,
    progression_signature: String,
}

#[derive(Debug, Serialize)]
struct SessionHeader {
    session_id: SessionId,
    generation: u32,
}

/// What the realm needs of a character: no extensions (opaque module payloads the realm cannot apply), only the settings the
/// policy carries.
pub fn job_model(
    model: &PortableCharacter,
    knowledge: Option<&super::knowledge::RealmKnowledge>,
) -> PortableCharacter {
    let mut m = model.clone();
    m.extensions.clear();
    // the selected appearances the realm's client data knows; without that knowledge none is sent (all stays canonical)
    m.wardrobe = match knowledge {
        Some(k) => m.wardrobe.restricted_to(|id| k.knows_appearance(id)),
        None => Default::default(),
    };
    m.settings
        .retain(|source, _| classify_setting(source, m.ruleset) == Disposition::Carry);
    m
}

/// The bytes of a job file: the header, a newline, the canonical JSON of the character.
pub fn job_bytes(
    job_id: ImportId,
    nonce: [u32; 4],
    account: u32,
    revision: u64,
    max_characters: u32,
    model: &PortableCharacter,
    session: Option<(SessionId, u32)>,
    knowledge: Option<&super::knowledge::RealmKnowledge>,
    pin: Option<&super::super::projection::ProgressionPin>,
) -> Result<Vec<u8>> {
    let snapshot_json = snapshot::canonical_json(&job_model(model, knowledge))?;
    let header = JobHeader {
        job_format: super::super::versions::ONLINE_IMPORT_JOB_FORMAT_VERSION,
        job_id,
        nonce,
        account,
        revision,
        max_characters_per_account: max_characters,
        snapshot_sha256: hex::encode(Sha256::digest(&snapshot_json)),
        character_id: model.character_id,
        session: session.map(|(session_id, generation)| SessionHeader {
            session_id,
            generation,
        }),
        projection: pin.map(|p| ProjectionHeader {
            active: p.projected,
            level: p.max_player_level,
            policy_version: p.policy_version,
            progression_signature: p.progression_signature.clone(),
        }),
    };
    let mut out = serde_json::to_vec(&header)?;
    out.push(b'\n');
    out.extend_from_slice(&snapshot_json);
    Ok(out)
}

fn job_paths(dir: &Path, job_id: ImportId) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{job_id}.job")),
        dir.join(format!("{job_id}.result")),
    )
}

/// What the core wrote into `<job_id>.result`.
#[derive(Debug, Deserialize)]
struct JobResult {
    status: String,
    #[serde(default)]
    local_guid: u32,
    #[serde(default)]
    item_base: u32,
    #[serde(default)]
    pet_base: u32,
    #[serde(default)]
    renamed: bool,
    #[serde(default)]
    final_name: String,
    #[serde(default)]
    items: usize,
    #[serde(default)]
    pets: usize,
    #[serde(default)]
    not_applied: Vec<String>,
    #[serde(default)]
    problems: Vec<JobProblem>,
    #[serde(default)]
    repeated: bool,
}

#[derive(Debug, Deserialize)]
struct JobProblem {
    code: String,
    detail: String,
}

/// Import the character's current canonical revision into a **running** realm through the core's import service.
pub fn import_character_online(
    ra: &mut Ra,
    store: &mut Store,
    character_id: CharacterId,
    server_id: &str,
    account: u32,
    opts: &ImportOptions,
    job_dir: &Path,
    session: Option<SessionId>,
) -> Result<ImportOutcome> {
    import_character_online_on(
        None,
        ra,
        store,
        character_id,
        server_id,
        account,
        opts,
        job_dir,
        session,
    )
}

/// The same, with the realm's database connection: the extensions of the character are then applied by their adapters after the core
/// committed the import (without it they stay canonical-only and are reported as not applied).
#[allow(clippy::too_many_arguments)]
pub fn import_character_online_on(
    db: Option<&crate::db::Db>,
    ra: &mut Ra,
    store: &mut Store,
    character_id: CharacterId,
    server_id: &str,
    account: u32,
    opts: &ImportOptions,
    job_dir: &Path,
    session: Option<SessionId>,
) -> Result<ImportOutcome> {
    let record = store.character(character_id)?;
    let canonical = store.load_snapshot(character_id, record.revision)?;
    if store
        .server_mappings(character_id)?
        .iter()
        .any(|m| m.server_id == server_id)
    {
        return Err(PortableError::AlreadyOnRealm {
            character: character_id,
            server_id: server_id.to_string(),
        });
    }
    let mut operations = vec![super::super::compat::Operation::OnlineImport];
    if session.is_some() {
        operations.push(super::super::compat::Operation::RuntimeSession);
    }
    let compatibility = super::profile::gate(opts, &canonical, &operations)?;
    let projection = {
        let oracle = super::project::CoreOracle::new(ra, job_dir);
        super::project::plan(&canonical, record.revision, opts, Some(&oracle))?
    };
    let model = projection.view.clone();
    let ticket = store.begin_import(
        character_id,
        server_id,
        record.revision,
        &planned_items(&model),
        &planned_pets(&model),
    )?;
    let (job_file, result_file) = job_paths(job_dir, ticket.import_id);
    let write = || -> Result<()> {
        let bytes = job_bytes(
            ticket.import_id,
            ticket.nonce,
            account,
            record.revision,
            opts.max_characters_per_account,
            &model,
            session.map(|s| (s, 1)),
            opts.knowledge.as_deref(),
            projection.pin.as_ref(),
        )?;
        let temp = job_file.with_extension("job.tmp");
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, &job_file)?;
        Ok(())
    };
    if let Err(e) = write() {
        store.abort_import(
            ticket.import_id,
            &format!("the job file could not be written: {e}"),
        )?;
        return Err(e);
    }

    let reply = ra.portable_import(ticket.import_id);
    let result: Option<JobResult> = std::fs::read(&result_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let cleanup = || {
        let _ = std::fs::remove_file(&job_file);
        let _ = std::fs::remove_file(&result_file);
    };
    match result {
        Some(r) if r.status == "ok" && r.local_guid != 0 => {
            let allocation = ImportAllocation {
                local_guid: r.local_guid,
                item_base: r.item_base,
                pet_base: r.pet_base,
            };
            if r.repeated {
                // the core found this job's marker: the allocation is read back from the realm by recovery
                cleanup();
                return finish_by_recovery(store, ticket.import_id, opts);
            }
            store.finish_import(ticket.import_id, allocation)?;
            super::project::remember(store, character_id, server_id, &projection)?;
            cleanup();
            let mut not_applied = r.not_applied;
            not_applied.extend(super::profile::after_write(
                db,
                store,
                character_id,
                server_id,
                opts,
                r.local_guid,
                &model,
                compatibility.as_ref(),
            )?);
            Ok(ImportOutcome {
                import_id: ticket.import_id,
                local_guid: r.local_guid,
                final_name: r.final_name,
                renamed: r.renamed,
                items: r.items,
                pets: r.pets,
                not_applied,
                warnings: vec![],
            })
        }
        Some(r) if r.status == "refused" => {
            let detail = r
                .problems
                .iter()
                .map(|p| format!("{}: {}", p.code, p.detail))
                .collect::<Vec<_>>()
                .join("; ");
            store.abort_import(
                ticket.import_id,
                &format!("the realm refused the import: {detail}"),
            )?;
            cleanup();
            Err(PortableError::ImportRefused(vec![
                super::import::ImportProblem::UnsupportedContent(detail),
            ]))
        }
        _ => {
            // no readable result: the marker in the realm decides, never the reply
            let why = reply
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "the realm wrote no result".into());
            Err(PortableError::ImportNeedsAttention {
                import_id: ticket.import_id,
                detail: format!("the outcome in the realm is not known yet ({why}); run recovery"),
            })
        }
    }
}

fn finish_by_recovery(
    _store: &mut Store,
    import_id: ImportId,
    _opts: &ImportOptions,
) -> Result<ImportOutcome> {
    Err(PortableError::ImportNeedsAttention {
        import_id,
        detail: "the core repeated an earlier job; run recovery to record it".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::fixtures::geared_level_eighty;
    use crate::portable::model::Extension;

    #[test]
    fn a_job_carries_what_the_realm_needs_and_nothing_else() {
        let mut model = geared_level_eighty();
        model.extensions.insert(
            "coa:unlisted-settings".into(),
            Extension::new("1", 1, vec![1, 2, 3]),
        );
        model.settings.insert("core.spell_charge.1".into(), vec![9]);
        model
            .settings
            .insert("core.ascension_build.54".into(), vec![1, 2]);
        let id = ImportId::new();
        let bytes = job_bytes(
            id,
            [1, 2, 3, 4],
            7001,
            5,
            10,
            &model,
            Some((SessionId::new(), 1)),
            None,
            None,
        )
        .unwrap();
        let split = bytes.iter().position(|b| *b == b'\n').unwrap();
        let header: serde_json::Value = serde_json::from_slice(&bytes[..split]).unwrap();
        let body = &bytes[split + 1..];
        assert_eq!(header["job_id"], id.to_string());
        assert_eq!(
            header["snapshot_sha256"],
            hex::encode(Sha256::digest(body)),
            "the hash is of the exact bytes of the snapshot line"
        );
        assert_eq!(header["nonce"], serde_json::json!([1, 2, 3, 4]));
        assert!(header["session"]["session_id"].is_string());
        assert!(!body.contains(&b'\n'), "the snapshot is one line");
        assert!(
            header["projection"].is_null(),
            "a job without a pin carries no projection header"
        );
        let sent: PortableCharacter = serde_json::from_slice(body).unwrap();
        assert!(
            sent.extensions.is_empty(),
            "opaque module payloads are not shipped to the realm"
        );
        assert!(
            sent.settings.contains_key("core.ascension_build.54")
                && !sent.settings.contains_key("core.spell_charge.1")
        );
        assert_eq!(sent.items.len(), model.items.len());
        assert!(
            job_bytes(id, [0; 4], 1, 1, 10, &model, None, None, None)
                .unwrap()
                .len()
                > 100
        );
    }
}
