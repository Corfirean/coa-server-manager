//! The real [`RealmBackend`]: the realm's login and character databases (read through the same `mysql` tools the rest of the Manager uses), its world console (for
//! creating accounts and setting passwords, the supported way) and this Manager's portable engine (for exporting characters).
//!
//! Account names are validated before they reach any statement. Passwords go to the console command and, for a link, into the computation of the realm's own SRP6
//! verifier to compare with the one stored; they are never written anywhere by this module.

use std::path::PathBuf;
use std::sync::Arc;

use coa_control_proto::app;

use super::service::{AccountRow, CharInfo, ClaimExport, Presence, RealmBackend, RealmInfo};
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
    pub fn new(portable: Arc<PortableRuntime>, settings_dir: impl Into<PathBuf>, descriptors: impl Into<PathBuf>, installs: Installs) -> Self {
        Self { portable, settings_dir: settings_dir.into(), descriptors: descriptors.into(), installs }
    }

    fn access(&self, local_id: &str) -> Result<RealmAccess> {
        find_access(&self.descriptors, &(self.installs)(), local_id).ok_or_else(|| Error::Invalid("That realm is not known to this Manager.".into()))
    }

    fn account(&self, local_id: &str, filter: &str) -> Result<Option<AccountRow>> {
        let db = self.access(local_id)?.db()?;
        let out = db.query(&format!("SELECT id, username FROM acore_auth.account WHERE {filter} LIMIT 1;"))?;
        Ok(out.lines().find(|l| !l.trim().is_empty()).and_then(|l| {
            let mut c = l.split('\t');
            Some(AccountRow { id: c.next()?.trim().parse().ok()?, username: c.next()?.trim().to_string() })
        }))
    }
}

impl RealmBackend for LocalBackend {
    fn info(&self, local_id: &str) -> Result<RealmInfo> {
        let cfg = settings::load(&self.settings_dir)?.realms.remove(local_id);
        Ok(RealmInfo { automatic: cfg.as_ref().is_none_or(|c| !c.existing_only), route: cfg.and_then(|c| c.route) })
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
        self.access(local_id)?.ra()?.set_account_password(&n, password)
    }

    fn check_login(&self, local_id: &str, username: &str, password: &str) -> Result<Option<AccountRow>> {
        let n = name(username)?;
        if is_internal(&n) || !app::valid_password_for_link(password) {
            return Ok(None);
        }
        let db = self.access(local_id)?.db()?;
        let out = db.query(&format!("SELECT id, username, HEX(salt), HEX(verifier) FROM acore_auth.account WHERE username = '{n}' LIMIT 1;"))?;
        let Some(line) = out.lines().find(|l| !l.trim().is_empty()) else { return Ok(None) };
        let c: Vec<&str> = line.split('\t').map(str::trim).collect();
        if c.len() < 4 {
            return Ok(None);
        }
        let (Ok(salt), Ok(stored)) = (hex::decode(c[2]), hex::decode(c[3])) else { return Ok(None) };
        let Ok(salt): std::result::Result<[u8; 32], _> = salt.try_into() else { return Ok(None) };
        let computed = crate::srp6::verifier(&n, password, &salt);
        // both are fixed-size secrets of equal length; compare without an early exit
        let equal = stored.len() == 32 && computed.iter().zip(&stored).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0;
        Ok(equal.then(|| AccountRow { id: c[0].parse().unwrap_or(0), username: c[1].to_string() }).filter(|r| r.id != 0))
    }

    fn presence(&self, local_id: &str, account_id: u32) -> Result<Presence> {
        let db = self.access(local_id)?.db()?;
        let out = db.query(&format!("SELECT (SELECT online FROM acore_auth.account WHERE id = {account_id}), (SELECT COUNT(*) FROM acore_characters.characters WHERE account = {account_id} AND online = 1);"))?;
        let line = out.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        let mut c = line.split('\t').map(str::trim);
        let account_online = c.next().is_some_and(|v| !v.is_empty() && v != "NULL" && v != "0");
        let online_characters = c.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        Ok(Presence { account_online, online_characters })
    }

    fn characters(&self, local_id: &str, account_id: u32) -> Result<Vec<CharInfo>> {
        let db = self.access(local_id)?.db()?;
        let list = realm::inspect_characters(&db).map_err(|e| unavailable("reading characters", e))?;
        Ok(list
            .into_iter()
            .filter(|c| c.account == account_id)
            .map(|c| CharInfo { guid: c.local_guid, name: c.name.clone(), class: c.class, race: c.race, level: c.level, eligible: c.eligible(), reasons: c.blockers.iter().map(|b| b.code().to_string()).collect() })
            .collect())
    }

    fn is_portable(&self, local_id: &str, guid: u32) -> Result<bool> {
        let id = local_id.to_string();
        self.portable.call(move |s| s.is_portable_here(&id, guid)).map_err(|e| unavailable("portable engine", e))?.map_err(|e| unavailable("portable engine", e))
    }

    fn export(&self, local_id: &str, _account_id: u32, guid: u32) -> Result<ClaimExport> {
        let id = local_id.to_string();
        let bundle = self.portable.call(move |s| s.export_for_claim(&id, guid)).map_err(|e| unavailable("portable engine", e))?.map_err(|e| unavailable("export", e))?;
        Ok(ClaimExport { character_id: bundle.character_id.as_uuid(), payload: bundle.payload, sha256: bundle.content_hash, collections: bundle.collections })
    }
}
