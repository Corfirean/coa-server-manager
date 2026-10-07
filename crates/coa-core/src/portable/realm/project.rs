//! The realm side of level-cap projection (Phase 8): deciding, for one canonical character and one realm, what the realm is given.
//!
//! ```text
//!   no capabilities                  nothing is evaluated: the character is given as it is (the lower layers' own tests, bare tools)
//!   capabilities without progression refused: the cap is unknown, and "unknown" is never read as "no cap"
//!   level <= cap                     native: given as it is, pinned to the realm's progression
//!   level >  cap                     projected: the realm's core decides what is held (a running core, or a decision supplied from it);
//!                                    the realm is given `apply(hold, canonical)` and the context is remembered
//! ```

use std::path::Path;
use std::sync::Mutex;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::ra::Ra;

use super::super::capabilities::RealmCapabilities;
use super::super::error::{PortableError, Result};
use super::super::ids::ImportId;
use super::super::model::PortableCharacter;
use super::super::projection::{activation, apply, subject_of, Activation, Decision, ProgressionPin, ProjectionAnswer, ProjectionContext, ProjectionOracle};
use super::super::snapshot;
use super::import::ImportOptions;
use super::online::job_model;

/// What a realm is given of a canonical character, and what is remembered of it.
#[derive(Debug, Clone)]
pub struct Plan {
    /// What the realm is given (`apply(hold, canonical)` when projected).
    pub view: PortableCharacter,
    /// `Some`: the character is projected on this realm.
    pub context: Option<ProjectionContext>,
    /// What the character is bound to on this realm; `None` only when the options carry no capabilities.
    pub pin: Option<ProgressionPin>,
}

impl Plan {
    pub fn projected(&self) -> bool {
        self.context.is_some()
    }
}

/// The plan for `canonical` (at canonical revision `revision`) on the realm the options describe.
pub fn plan(canonical: &PortableCharacter, revision: u64, opts: &ImportOptions, live: Option<&dyn ProjectionOracle>) -> Result<Plan> {
    plan_with(canonical, revision, opts.capabilities.as_deref(), live.or(opts.projection.as_ref().map(|o| &*o.0)))
}

pub fn plan_with(canonical: &PortableCharacter, revision: u64, caps: Option<&RealmCapabilities>, oracle: Option<&dyn ProjectionOracle>) -> Result<Plan> {
    let Some(caps) = caps else { return Ok(Plan { view: canonical.clone(), context: None, pin: None }) };
    let progression = caps.progression.as_ref().ok_or_else(|| PortableError::ProgressionChanged("the realm's progression profile is not known: probe the realm through its core first".into()))?;
    match activation(canonical.progression.level, progression) {
        Activation::None => Ok(Plan { view: canonical.clone(), context: None, pin: Some(ProgressionPin::native(progression, &caps.content_profile_hash)) }),
        Activation::Active { canonical_level, projected_level } => {
            let oracle = oracle.ok_or(PortableError::ProjectionNeedsRunningCore { character_level: canonical_level, cap: projected_level })?;
            let Decision::Projected(hold) = oracle.decide(canonical)? else {
                return Err(PortableError::ProgressionChanged("the core does not project a character the Manager sees above its cap".into()));
            };
            if hold.progression_signature != progression.progression_signature || hold.max_player_level != progression.max_player_level || hold.policy_version != progression.projection_policy_version {
                return Err(PortableError::ProgressionChanged("the core decided under another progression profile than the one the realm reported".into()));
            }
            hold.check_subject(canonical)?;
            let view = apply(canonical, &hold);
            let context = ProjectionContext::new(hold, revision, &caps.content_profile_hash);
            context.validate()?;
            let pin = Some(context.pin());
            Ok(Plan { view, context: Some(context), pin })
        }
    }
}

/// The header of a projection query (a job file with no import in it).
#[derive(Debug, Serialize)]
struct QueryHeader {
    job_format: u32,
    job_id: ImportId,
    query: &'static str,
    snapshot_sha256: String,
}

fn query_bytes(job_id: ImportId, model: &PortableCharacter) -> Result<Vec<u8>> {
    let snapshot_json = snapshot::canonical_json(&job_model(model, None))?;
    let header = QueryHeader { job_format: super::super::versions::ONLINE_IMPORT_JOB_FORMAT_VERSION, job_id, query: "project", snapshot_sha256: hex::encode(Sha256::digest(&snapshot_json)) };
    let mut out = serde_json::to_vec(&header)?;
    out.push(b'\n');
    out.extend_from_slice(&snapshot_json);
    Ok(out)
}

/// Ask a running core what a projection of this character holds (RA `portable project`): a job file in the core's job directory, only its
/// id through the console, the answer in the job's result file.
pub fn decide_with_core(ra: &mut Ra, job_dir: &Path, canonical: &PortableCharacter) -> Result<Decision> {
    let id = ImportId::new();
    let job = job_dir.join(format!("{id}.job"));
    let result = job_dir.join(format!("{id}.result"));
    let temp = job.with_extension("job.tmp");
    std::fs::write(&temp, query_bytes(id, canonical)?)?;
    std::fs::rename(&temp, &job)?;
    let reply = ra.portable_project(id);
    let answer = std::fs::read_to_string(&result);
    let _ = std::fs::remove_file(&job);
    let _ = std::fs::remove_file(&result);
    let text = match (&reply, answer) {
        (_, Ok(text)) => text,
        (Err(e), Err(_)) => return Err(PortableError::RealmRead(e.to_string())),
        (Ok(_), Err(e)) => return Err(PortableError::RealmRead(format!("the core wrote no projection answer: {e}"))),
    };
    ProjectionAnswer::parse(&text, &subject_of(canonical)?)
}

/// A running realm as an oracle.
pub struct CoreOracle<'a> {
    ra: Mutex<&'a mut Ra>,
    job_dir: std::path::PathBuf,
}

impl<'a> CoreOracle<'a> {
    pub fn new(ra: &'a mut Ra, job_dir: &Path) -> Self {
        Self { ra: Mutex::new(ra), job_dir: job_dir.to_path_buf() }
    }
}

impl ProjectionOracle for CoreOracle<'_> {
    fn decide(&self, canonical: &PortableCharacter) -> Result<Decision> {
        let mut ra = self.ra.lock().map_err(|_| PortableError::Invalid("the console is unavailable".into()))?;
        decide_with_core(&mut ra, &self.job_dir, canonical)
    }
}

/// Remember what was written to a realm: the projection's context when the character is projected there, else that it is native, and the
/// pin it is bound to. Call after the realm committed (the mapping must exist).
pub fn remember(store: &mut super::super::store::Store, id: super::super::ids::CharacterId, server_id: &str, plan: &Plan) -> Result<()> {
    match &plan.context {
        Some(ctx) => store.set_projection_context(id, server_id, ctx),
        None => {
            store.clear_projection_context(id, server_id)?;
            store.set_mapping_pin(id, server_id, plan.pin.as_ref())
        }
    }
}

/// What a stored projection context shows of a (later) canonical state: the same decision applied again.
pub fn stored_view(state: &PortableCharacter, context: Option<&ProjectionContext>) -> PortableCharacter {
    match context {
        Some(c) => apply(state, &c.hold),
        None => state.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::capabilities::*;
    use crate::portable::fixtures::geared_level_eighty;
    use crate::portable::model::Ruleset;
    use crate::portable::projection::{ProjectionHold, SuppliedDecision, PROTOCOL};

    const SIG: &str = "bd299b9ef5863c6bf006e88a3617fac57c514b5889ea8367f87879aa1ad6d76d";

    pub(crate) fn caps(cap: u32, signature: &str) -> RealmCapabilities {
        let content = ContentProfile {
            ruleset: Ruleset::Coa,
            character_formats: ContentProfile::manager_formats(),
            online_import_job_formats: vec![2],
            session_protocol: 2,
            collection_protocol: 2,
            features: [Feature::RuntimeSessions, Feature::LevelProjection].into_iter().collect(),
            collection_kinds: Default::default(),
            extensions: vec![],
            client_catalog: Default::default(),
        };
        RealmCapabilities::build(None, content, Some(Progression { max_player_level: cap, projection_protocol: PROTOCOL, projection_policy_version: 1, progression_signature: signature.into(), scaling_enabled: true })).unwrap()
    }

    fn hold(c: &PortableCharacter, cap: u32, signature: &str) -> ProjectionHold {
        ProjectionHold { protocol: PROTOCOL, policy_version: 1, progression_signature: signature.into(), max_player_level: cap, canonical_level: c.progression.level as u32, projected_level: cap, held_items: vec![c.items[0].id], held_spells: vec![], held_actions: vec![], settings: vec![], blocked_settings: vec![], subject: subject_of(c).unwrap() }
    }

    #[test]
    fn a_character_within_the_cap_is_given_as_it_is_and_one_above_needs_a_decision() {
        let c = geared_level_eighty();
        let native = plan_with(&c, 3, Some(&caps(80, SIG)), None).unwrap();
        assert!(!native.projected());
        assert_eq!(native.view, c);
        assert!(!native.pin.unwrap().projected);

        let err = plan_with(&c, 3, Some(&caps(60, SIG)), None).unwrap_err();
        assert!(matches!(err, PortableError::ProjectionNeedsRunningCore { character_level: 80, cap: 60 }), "{err}");

        let supplied = SuppliedDecision(hold(&c, 60, SIG));
        let p = plan_with(&c, 3, Some(&caps(60, SIG)), Some(&supplied)).unwrap();
        assert!(p.projected());
        assert_eq!(p.view.progression.level, 60);
        assert_eq!(p.view.items.len(), c.items.len() - 1);
        let ctx = p.context.unwrap();
        assert_eq!((ctx.canonical_level, ctx.projected_level, ctx.canonical_revision), (80, 60, 3));
        assert!(p.pin.unwrap().projected);
        assert!(plan_with(&c, 3, None, None).unwrap().pin.is_none(), "without a profile nothing is evaluated");
    }

    #[test]
    fn a_decision_of_another_progression_or_another_state_is_refused() {
        let c = geared_level_eighty();
        let other = "ab".repeat(32);
        let wrong_profile = SuppliedDecision(hold(&c, 60, &other));
        assert!(matches!(plan_with(&c, 3, Some(&caps(60, SIG)), Some(&wrong_profile)), Err(PortableError::ProgressionChanged(_))));
        let wrong_cap = SuppliedDecision(hold(&c, 70, SIG));
        assert!(matches!(plan_with(&c, 3, Some(&caps(60, SIG)), Some(&wrong_cap)), Err(PortableError::ProgressionChanged(_))));
        let mut changed = c.clone();
        changed.progression.money += 1;
        let stale = SuppliedDecision(hold(&c, 60, SIG));
        assert!(plan_with(&changed, 3, Some(&caps(60, SIG)), Some(&stale)).is_err(), "made for another state of the character");
        let mut unnamed = hold(&c, 60, SIG);
        unnamed.subject.clear();
        assert!(plan_with(&c, 3, Some(&caps(60, SIG)), Some(&SuppliedDecision(unnamed))).is_err());
    }

    #[test]
    fn a_profile_without_progression_is_never_read_as_no_cap() {
        let c = geared_level_eighty();
        let mut unknown = caps(80, SIG);
        unknown.progression = None;
        unknown.content.features.remove(&Feature::LevelProjection);
        unknown.content_profile_hash = hex::encode(unknown.content.hash().unwrap());
        assert!(matches!(plan_with(&c, 3, Some(&unknown), None), Err(PortableError::ProgressionChanged(_))));
    }

    #[test]
    fn a_query_job_names_its_character_and_its_hash() {
        let c = geared_level_eighty();
        let id = ImportId::new();
        let bytes = query_bytes(id, &c).unwrap();
        let split = bytes.iter().position(|b| *b == b'\n').unwrap();
        let header: serde_json::Value = serde_json::from_slice(&bytes[..split]).unwrap();
        assert_eq!(header["query"], "project");
        assert_eq!(header["job_format"], 2);
        assert_eq!(header["snapshot_sha256"], hex::encode(Sha256::digest(&bytes[split + 1..])));
        assert!(header.get("account").is_none() && header.get("nonce").is_none(), "a query writes nothing and carries no import data");
    }
}
