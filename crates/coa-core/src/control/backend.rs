//! The real [`RealmBackend`]: the realm's login and character databases (read through the same `mysql` tools the rest of the Manager uses), its world console (for
//! creating accounts and setting passwords, the supported way) and this Manager's portable engine (for exporting characters).
//!
//! Account names are validated before they reach any statement. Passwords go to the console command and, for a link, into the computation of the realm's own SRP6
//! verifier to compare with the one stored; they are never written anywhere by this module.

use std::path::PathBuf;
use std::sync::Arc;

use coa_control_proto::app;

use uuid::Uuid;

use super::service::{
    AccountRow, CharInfo, ClaimExport, PreflightVerdict, Presence, RealmBackend, RealmInfo,
    RemoteImportOutcome, RemoteImportParams,
};
use crate::portable::realm;
use crate::portable::service::access::RealmAccess;
use crate::portable::service::runtime::PortableRuntime;
use crate::realm_registry::advert::find_access;
use crate::realm_registry::settings;
use crate::{Error, Result};

pub type Installs = Arc<dyn Fn() -> Vec<(String, PathBuf)> + Send + Sync>;

pub struct LocalBackend {
    portable: Arc<PortableRuntime>,
    /// Folder of `registry.json` (the owner's choices per realm: existing accounts only, a stated route).
    settings_dir: PathBuf,
    descriptors: PathBuf,
    installs: Installs,
}

fn unavailable(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("{what}: {e}"))
}

fn name(username: &str) -> Result<String> {
    let upper = username.to_ascii_uppercase();
    if !app::valid_username(&upper) {
        return Err(Error::Invalid("That is not a game account name.".into()));
    }
    Ok(upper)
}

fn is_internal(upper: &str) -> bool {
    upper == "COAMANAGER" || upper.starts_with("COABOT")
}

impl LocalBackend {
    pub fn new(
        portable: Arc<PortableRuntime>,
        settings_dir: impl Into<PathBuf>,
        descriptors: impl Into<PathBuf>,
        installs: Installs,
    ) -> Self {
        Self {
            portable,
            settings_dir: settings_dir.into(),
            descriptors: descriptors.into(),
            installs,
        }
    }

    fn require_portable(&self, local_id: &str) -> Result<()> {
        let settings = settings::load(&self.settings_dir)?;
        if settings.realms.get(local_id).is_some_and(|cfg| cfg.allow_portable) {
            Ok(())
        } else {
            Err(Error::Invalid("The realm owner has disabled portable characters.".into()))
        }
    }

    fn access(&self, local_id: &str) -> Result<RealmAccess> {
        let access =
            find_access(&self.descriptors, &(self.installs)(), local_id).ok_or_else(|| {
                Error::Invalid(format!(
                    "That realm is not known to this Manager: {local_id}"
                ))
            })?;
        if let Some((_base, rs)) = local_id.split_once(':') {
            let expected_mode = match rs {
                "coa" => crate::realms::Mode::Coa,
                "wildcard" => crate::realms::Mode::Wildcard,
                "reborn" => crate::realms::Mode::Reborn,
                _ => {
                    return Err(Error::Invalid(format!(
                        "Malformed or unknown ruleset suffix in local_id: {rs}"
                    )))
                }
            };
            if access.mode != Some(expected_mode) {
                return Err(Error::Invalid(format!(
                    "Realm access mode {:?} does not match expected ruleset in local_id {local_id}",
                    access.mode
                )));
            }
        }
        Ok(access)
    }

    fn account(&self, local_id: &str, filter: &str) -> Result<Option<AccountRow>> {
        let db = self.access(local_id)?.db()?;
        let out = db.query(&format!(
            "SELECT id, username FROM acore_auth.account WHERE {filter} LIMIT 1;"
        ))?;
        Ok(out.lines().find(|l| !l.trim().is_empty()).and_then(|l| {
            let mut c = l.split('\t');
            Some(AccountRow {
                id: c.next()?.trim().parse().ok()?,
                username: c.next()?.trim().to_string(),
            })
        }))
    }
}

impl RealmBackend for LocalBackend {
    fn info(&self, local_id: &str) -> Result<RealmInfo> {
        let cfg = settings::load(&self.settings_dir)?.realms.remove(local_id);
        let access = self.access(local_id)?;
        let db = access.db()?;
        let ruleset = realm::ruleset_of(&db).map_err(|e| Error::Invalid(e.to_string()))?;

        // Explicit local-id -> ruleset invariant:
        // local_id ruleset == RealmAccess.mode == Db.realm() == RealmInfo.ruleset
        if let Some((_base, rs)) = local_id.split_once(':') {
            let expected_ruleset: crate::portable::Ruleset = rs
                .parse()
                .map_err(|_| Error::Invalid(format!("Unknown ruleset in local_id: {rs}")))?;
            if ruleset != expected_ruleset {
                return Err(Error::Invalid(format!(
                    "Ruleset of realm DB ({ruleset:?}) does not match local_id {local_id}"
                )));
            }
            if access.mode.map(|m| m.name()) != Some(expected_ruleset.as_str()) {
                return Err(Error::Invalid(format!(
                    "RealmAccess mode ({:?}) does not match local_id {local_id}",
                    access.mode
                )));
            }
            if db.realm().name() != expected_ruleset.as_str() {
                return Err(Error::Invalid(format!(
                    "Db realm ({:?}) does not match local_id {local_id}",
                    db.realm()
                )));
            }
        }

        Ok(RealmInfo {
            automatic: cfg.as_ref().is_none_or(|c| !c.existing_only),
            route: cfg.and_then(|c| c.route),
            ruleset,
        })
    }

    fn account_by_name(&self, local_id: &str, username: &str) -> Result<Option<AccountRow>> {
        let n = name(username)?;
        self.account(local_id, &format!("username = '{n}'"))
    }

    fn account_by_id(&self, local_id: &str, id: u32) -> Result<Option<AccountRow>> {
        self.account(local_id, &format!("id = {id}"))
    }

    fn create_account(&self, local_id: &str, username: &str, password: &str) -> Result<()> {
        let n = name(username)?;
        if is_internal(&n) {
            return Err(Error::Invalid("That name is reserved.".into()));
        }
        self.access(local_id)?.ra()?.create_account(&n, password)
    }

    fn set_password(&self, local_id: &str, username: &str, password: &str) -> Result<()> {
        let n = name(username)?;
        if is_internal(&n) {
            return Err(Error::Invalid("That name is reserved.".into()));
        }
        self.access(local_id)?
            .ra()?
            .set_account_password(&n, password)
    }

    fn check_login(
        &self,
        local_id: &str,
        username: &str,
        password: &str,
    ) -> Result<Option<AccountRow>> {
        let n = name(username)?;
        if is_internal(&n) || !app::valid_password_for_link(password) {
            return Ok(None);
        }
        let db = self.access(local_id)?.db()?;
        let out = db.query(&format!("SELECT id, username, HEX(salt), HEX(verifier) FROM acore_auth.account WHERE username = '{n}' LIMIT 1;"))?;
        let Some(line) = out.lines().find(|l| !l.trim().is_empty()) else {
            return Ok(None);
        };
        let c: Vec<&str> = line.split('\t').map(str::trim).collect();
        if c.len() < 4 {
            return Ok(None);
        }
        let (Ok(salt), Ok(stored)) = (hex::decode(c[2]), hex::decode(c[3])) else {
            return Ok(None);
        };
        let Ok(salt): std::result::Result<[u8; 32], _> = salt.try_into() else {
            return Ok(None);
        };
        let computed = crate::srp6::verifier(&n, password, &salt);
        // both are fixed-size secrets of equal length; compare without an early exit
        let equal = stored.len() == 32
            && computed
                .iter()
                .zip(&stored)
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0;
        Ok(equal
            .then(|| AccountRow {
                id: c[0].parse().unwrap_or(0),
                username: c[1].to_string(),
            })
            .filter(|r| r.id != 0))
    }

    fn presence(&self, local_id: &str, account_id: u32) -> Result<Presence> {
        let db = self.access(local_id)?.db()?;
        let out = db.query(&format!("SELECT (SELECT online FROM acore_auth.account WHERE id = {account_id}), (SELECT COUNT(*) FROM acore_characters.characters WHERE account = {account_id} AND online = 1);"))?;
        let line = out.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let mut c = line.split('\t').map(str::trim);
        let account_online = c
            .next()
            .is_some_and(|v| !v.is_empty() && v != "NULL" && v != "0");
        let online_characters = c.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        Ok(Presence {
            account_online,
            online_characters,
        })
    }

    fn characters(&self, local_id: &str, account_id: u32) -> Result<Vec<CharInfo>> {
        let db = self.access(local_id)?.db()?;
        let list =
            realm::inspect_characters(&db).map_err(|e| unavailable("reading characters", e))?;
        Ok(list
            .into_iter()
            .filter(|c| c.account == account_id)
            .map(|c| CharInfo {
                guid: c.local_guid,
                name: c.name.clone(),
                class: c.class,
                race: c.race,
                level: c.level,
                eligible: c.eligible(),
                reasons: c.blockers.iter().map(|b| b.code().to_string()).collect(),
            })
            .collect())
    }

    fn is_portable(&self, local_id: &str, guid: u32) -> Result<bool> {
        let id = local_id.to_string();
        self.portable
            .call(move |s| s.is_portable_here(&id, guid))
            .map_err(|e| unavailable("portable engine", e))?
            .map_err(|e| unavailable("portable engine", e))
    }

    fn export(&self, local_id: &str, _account_id: u32, guid: u32) -> Result<ClaimExport> {
        self.require_portable(local_id)?;
        let id = local_id.to_string();
        let bundle = self
            .portable
            .call(move |s| s.export_for_claim(&id, guid))
            .map_err(|e| unavailable("portable engine", e))?
            .map_err(|e| unavailable("export", e))?;
        Ok(ClaimExport {
            character_id: bundle.character_id.as_uuid(),
            ruleset: bundle.ruleset,
            payload: bundle.payload,
            sha256: bundle.content_hash,
            collections: bundle.collections,
        })
    }

    fn check_transfer(
        &self,
        local_id: &str,
        character_id: Uuid,
        revision: u64,
    ) -> Result<PreflightVerdict> {
        self.require_portable(local_id)?;
        let id = local_id.to_string();
        let cid = crate::portable::CharacterId::from_uuid(character_id)
            .map_err(|e| unavailable("character id", e))?;
        self.portable
            .call(move |s| s.check_remote_transfer(&id, cid, revision))
            .map_err(|e| unavailable("portable engine", e))?
            .map_err(|e| unavailable("check transfer", e))
    }

    fn import_remote(
        &self,
        local_id: &str,
        params: RemoteImportParams,
    ) -> Result<RemoteImportOutcome> {
        self.require_portable(local_id)?;
        let id = local_id.to_string();
        self.portable
            .call(move |s| s.import_remote_character(&id, params))
            .map_err(|e| unavailable("portable engine", e))?
            .map_err(|e| unavailable("import remote", e))
    }

    fn world_port(&self, local_id: &str) -> Result<u16> {
        if let Ok(override_port) = std::env::var("COA_OVERRIDE_LOCAL_WORLD_PORT") {
            if let Ok(p) = override_port.parse::<u16>() {
                return Ok(p);
            }
        }
        let access = self.access(local_id)?;
        let mode = if let Some(m) = access.mode {
            m
        } else if let Some((_base, _rs)) = local_id.split_once(':') {
            return Err(Error::Invalid(format!(
                "Unresolved mode for ruleset suffix in {local_id}"
            )));
        } else {
            // Legacy unsuffixed CoA realm ONLY
            crate::realms::Mode::Coa
        };
        if let Some(ref root) = access.root {
            crate::realms::world_port_for_mode(root, mode)
        } else {
            Err(Error::Invalid(format!(
                "No server installation root for {local_id} to resolve world port"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn portable_access_requires_owner_opt_in_and_revokes_immediately() {
        let dir = tempdir().unwrap();
        let rt = PortableRuntime::start(&dir.path().join("portable"), Box::new(Vec::new), None).unwrap();
        let backend = LocalBackend::new(Arc::new(rt), dir.path(), dir.path(), Arc::new(Vec::new));
        assert!(backend.require_portable("srv-test").is_err());
        let mut cfg = settings::RegistrySettings::default();
        cfg.realms.insert("srv-test".into(), settings::PublishConfig {
            realm_id: None, enabled: true, display_name: "Test".into(), description: String::new(),
            language: "en".into(), region: None, existing_only: false, allow_portable: true, route: None,
        });
        settings::save(dir.path(), &cfg).unwrap();
        assert!(backend.require_portable("srv-test").is_ok());
        cfg.realms.get_mut("srv-test").unwrap().allow_portable = false;
        settings::save(dir.path(), &cfg).unwrap();
        assert!(backend.export("srv-test", 1, 1).unwrap_err().to_string().contains("disabled portable"));
        assert!(backend.check_transfer("srv-test", Uuid::new_v4(), 1).unwrap_err().to_string().contains("disabled portable"));
    }

    #[test]
    fn unreadable_or_unknown_realm_fails_closed_in_info() {
        let dir = tempdir().unwrap();
        let settings = dir.path().join("Settings");
        fs::create_dir_all(&settings).unwrap();
        fs::write(settings.join("registry.json"), "{}").unwrap();

        let desc_dir = dir.path().join("descriptors");
        fs::create_dir_all(&desc_dir).unwrap();

        let portable_dir = dir.path().join("portable");
        let rt = PortableRuntime::start(&portable_dir, Box::new(Vec::new), None).unwrap();
        let backend = LocalBackend::new(Arc::new(rt), &settings, &desc_dir, Arc::new(Vec::new));

        // Unknown realm id must return Err, never Ok(RealmInfo { ruleset: Ruleset::Coa, .. })
        assert!(backend.info("srv-unknown").is_err());
        assert!(backend.info("srv-unknown:wildcard").is_err());
        assert!(backend.info("srv-unknown:reborn").is_err());
    }

    #[test]
    fn world_port_for_mode_distinguishes_simultaneous_realms() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("srv");
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::write(root.join("Settings/repack.json"), r#"{"worldPort": 8085}"#).unwrap();
        fs::write(
            root.join("Settings/realm-profile.json"),
            r#"{"active": "coa", "wildcard_created": true, "simultaneous": true, "secondary_world_port": 8086}"#,
        )
        .unwrap();

        assert_eq!(
            crate::realms::world_port_for_mode(&root, crate::realms::Mode::Coa).unwrap(),
            8085
        );
        assert_eq!(
            crate::realms::world_port_for_mode(&root, crate::realms::Mode::Wildcard).unwrap(),
            8086
        );

        let desc_dir = dir.path().join("descriptors");
        let portable_dir = dir.path().join("portable");
        let rt = PortableRuntime::start(&portable_dir, Box::new(Vec::new), None).unwrap();
        let root_clone = root.clone();
        let backend = LocalBackend::new(
            Arc::new(rt),
            root.join("Settings"),
            &desc_dir,
            Arc::new(move || vec![("test1".to_string(), root_clone.clone())]),
        );

        assert_eq!(backend.world_port("srv-test1:coa").unwrap(), 8085);
        assert_eq!(backend.world_port("srv-test1:wildcard").unwrap(), 8086);

        // When simultaneous is false and active is CoA:
        fs::write(
            root.join("Settings/realm-profile.json"),
            r#"{"active": "coa", "wildcard_created": true, "simultaneous": false}"#,
        )
        .unwrap();

        // CoA succeeds with 8085
        assert_eq!(backend.world_port("srv-test1:coa").unwrap(), 8085);
        // Wildcard must FAIL CLOSED, never return 8085
        let err_wc_mode =
            crate::realms::world_port_for_mode(&root, crate::realms::Mode::Wildcard).unwrap_err();
        assert!(
            err_wc_mode.to_string().contains("not active"),
            "Expected not active error, got: {err_wc_mode}"
        );
        assert!(backend.world_port("srv-test1:wildcard").is_err());

        // Reborn must FAIL CLOSED
        let err_reb_mode =
            crate::realms::world_port_for_mode(&root, crate::realms::Mode::Reborn).unwrap_err();
        assert!(
            err_reb_mode.to_string().contains("not active"),
            "Expected not active error, got: {err_reb_mode}"
        );
        assert!(backend.world_port("srv-test1:reborn").is_err());

        // When active is Reborn:
        fs::write(
            root.join("Settings/realm-profile.json"),
            r#"{"active": "reborn", "wildcard_created": true, "reborn_created": true, "simultaneous": false}"#,
        )
        .unwrap();

        // Reborn succeeds with primary world port 8085
        assert_eq!(backend.world_port("srv-test1:reborn").unwrap(), 8085);
        // CoA must FAIL CLOSED when Reborn is active
        assert!(backend.world_port("srv-test1:coa").is_err());
        // Wildcard must FAIL CLOSED when Reborn is active
        assert!(backend.world_port("srv-test1:wildcard").is_err());

        // Malformed and unknown suffixes must fail closed
        assert!(backend.access("srv-test1:unknown").is_err());
        assert!(backend.access("srv-test1:garbage").is_err());
        assert!(backend.access("srv-test1:coa:extra").is_err());
        assert!(backend.world_port("srv-test1:unknown").is_err());
        assert!(backend.world_port("srv-test1:garbage").is_err());
    }

    #[test]
    fn world_port_lookup_failure_causes_direct_and_relay_to_fail_closed() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("srv");
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::write(root.join("Settings/repack.json"), r#"{"worldPort": 8085}"#).unwrap();
        // Server only has active CoA, simultaneous is off
        fs::write(
            root.join("Settings/realm-profile.json"),
            r#"{"active": "coa", "wildcard_created": false, "simultaneous": false}"#,
        )
        .unwrap();

        let desc_dir = dir.path().join("descriptors");
        let portable_dir = dir.path().join("portable");
        let rt = PortableRuntime::start(&portable_dir, Box::new(Vec::new), None).unwrap();
        let root_clone = root.clone();
        let backend = Arc::new(LocalBackend::new(
            Arc::new(rt),
            root.join("Settings"),
            &desc_dir,
            Arc::new(move || vec![("test1".to_string(), root_clone.clone())]),
        ));

        let store = Arc::new(std::sync::Mutex::new(
            crate::control::store::ControlStore::open_in_memory().unwrap(),
        ));
        let service = Arc::new(crate::control::service::HostService::new(
            "srv-test1:wildcard",
            coa_registry_proto::sign::RealmId::new(),
            store,
            backend,
        ));

        // 1. HostService::local_world_port fails closed
        assert!(service.local_world_port().is_err());

        // 2. Direct route manager cannot find a direct route because world port resolution failed
        let mut direct_mgr =
            crate::control::direct_route::DirectRouteManager::start(None, service.clone());
        assert!(!direct_mgr.is_direct_ready());
        assert!(service.direct_route().is_none());
        direct_mgr.shutdown();

        // 3. When relay link has no world port resolved, AllocateRelay refuses
        let mut session = crate::control::service::Session::default();
        let player = uuid::Uuid::now_v7();
        let resp = service.handle(
            &mut session,
            &player,
            "KEY",
            coa_control_proto::app::Request::AllocateRelay { player_id: player },
        );
        assert!(matches!(
            resp,
            coa_control_proto::app::Response::Error {
                code: coa_control_proto::app::AppError::NoAccount
                    | coa_control_proto::app::AppError::RelayUnavailable,
                ..
            }
        ));
    }
}
