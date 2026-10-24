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
        if settings
            .realms
            .get(local_id)
            .is_some_and(|cfg| cfg.allow_portable)
        {
            Ok(())
        } else {
            Err(Error::Invalid(
                "The realm owner has disabled portable characters.".into(),
            ))
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
    fn session_message(
        &self,
        local_id: &str,
        character: Uuid,
        session: Uuid,
        account: u32,
    ) -> Result<super::service::RemoteSessionState> {
        let id = local_id.to_string();
        self.portable
            .call(move |service| service.remote_session_message(&id, character, session, account))
            .map_err(|error| unavailable("portable engine", error))?
            .map_err(|error| unavailable("portable session", error))
    }

    fn session_ack(
        &self,
        local_id: &str,
        character: Uuid,
        session: Uuid,
        account: u32,
        kind: app::SessionMessageKind,
        bytes: &[u8],
    ) -> Result<()> {
        let id = local_id.to_string();
        let bytes = bytes.to_vec();
        self.portable
            .call(move |service| {
                service.apply_remote_session_ack(&id, character, session, account, kind, &bytes)
            })
            .map_err(|error| unavailable("portable engine", error))?
            .map_err(|error| unavailable("portable session", error))
    }
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

    fn auth_port(&self, local_id: &str) -> Result<u16> {
        let access = self.access(local_id)?;
        let root = access
            .root
            .ok_or_else(|| Error::Invalid("The realm's auth port is unresolved.".into()))?;
        let settings: serde_json::Value =
            crate::fsx::read_json(&root.join("Settings/repack.json"))?;
        match settings.get("authPort") {
            None => Ok(coa_control_proto::relay::LOCAL_AUTH_PORT),
            Some(value) => value
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .ok_or_else(|| Error::Invalid("The realm's auth port is invalid.".into())),
        }
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
        let rt =
            PortableRuntime::start(&dir.path().join("portable"), Box::new(Vec::new), None).unwrap();
        let backend = LocalBackend::new(Arc::new(rt), dir.path(), dir.path(), Arc::new(Vec::new));
        assert!(backend.require_portable("srv-test").is_err());
        let mut cfg = settings::RegistrySettings::default();
        cfg.realms.insert(
            "srv-test".into(),
            settings::PublishConfig {
                realm_id: None,
                enabled: true,
                display_name: "Test".into(),
                description: String::new(),
                language: "en".into(),
                region: None,
                existing_only: false,
                allow_portable: true,
                route: None,
            },
        );
        settings::save(dir.path(), &cfg).unwrap();
        assert!(backend.require_portable("srv-test").is_ok());
        cfg.realms.get_mut("srv-test").unwrap().allow_portable = false;
        settings::save(dir.path(), &cfg).unwrap();
        assert!(backend
            .export("srv-test", 1, 1)
            .unwrap_err()
            .to_string()
            .contains("disabled portable"));
        assert!(backend
            .check_transfer("srv-test", Uuid::new_v4(), 1)
            .unwrap_err()
            .to_string()
            .contains("disabled portable"));
    }

    #[test]
    #[ignore = "Requires disposable A/B databases, B worldserver configuration and REGISTRY_URL"]
    fn live_vps_transfer_uses_real_backend_and_preserves_the_working_copy() {
        use crate::control::{
            host_link::HostLink,
            join::{PlayerControl, RealmTarget},
            secrets::MemoryStore,
            service::HostService,
            store::ControlStore,
            transport::coordinator_url,
        };
        use crate::portable::{
            extension::ExtensionRegistry,
            realm::{
                live_import::{number, realms, reset_b},
                live_projection::fixture_character,
                profile::probe_capabilities,
            },
            session::live_session::{spec, Server},
            store::Store,
        };
        use crate::realm_registry::{advert::advertise, client::RegistryClient};
        use coa_registry_proto::{
            AccountProvisioning, Listing, Population, Rates, RealmId, RegisterRequest,
            REGISTRY_PROTOCOL_VERSION,
        };
        use std::collections::BTreeMap;
        use std::sync::Mutex;
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        let base = std::env::var("REGISTRY_URL").expect("REGISTRY_URL");
        let root = std::path::PathBuf::from(
            std::env::var("COA_REAL_CONTROL_ROOT").expect("disposable root"),
        );
        assert!(root.starts_with("C:/games/source/candidate-evidence-20261021"));
        crate::config::portable::enable(&root).expect("owner portable-character opt-in");
        let databases = realms().expect("disposable databases");
        reset_b(&databases.b);
        let server = Server::start(spec().expect("disposable worldserver"));
        struct Auth(std::process::Child);
        impl Drop for Auth {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let auth_port = crate::layout::read_ports(&root).auth;
        assert_eq!(
            auth_port, 57637,
            "Only the disposable auth port may be used"
        );
        assert!(std::net::TcpStream::connect(("127.0.0.1", auth_port)).is_err());
        let mut command = std::process::Command::new(root.join("Core/authserver.exe"));
        command
            .arg("-c")
            .arg(root.join("Core/configs/authserver.conf"))
            .current_dir(root.join("Core"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut auth = Auth(command.spawn().unwrap());
        let end = Instant::now() + Duration::from_secs(30);
        while std::net::TcpStream::connect(("127.0.0.1", auth_port)).is_err() {
            assert!(
                auth.0.try_wait().unwrap().is_none(),
                "Disposable authserver exited"
            );
            assert!(Instant::now() < end, "Disposable authserver did not listen");
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut ra = crate::ra::Ra::connect_to(
            server.spec.ra_port,
            &server.spec.user,
            &server.spec.password,
        )
        .unwrap();
        let caps = probe_capabilities(
            &databases.b,
            Some(&root.join("Data")),
            Some(&mut ra),
            &ExtensionRegistry::new(),
        )
        .unwrap();
        let advertised = advertise(&caps).unwrap();
        let realm = RealmId::new();
        let mut seed = [0u8; 32];
        seed[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        seed[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let clock = || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
        };
        let registry = RegistryClient::new(&base).unwrap();
        let listing = Listing {
            display_name: "Real portable candidate gate".into(),
            description: "Disposable database and real worldserver test".into(),
            language: "en".into(),
            region: None,
            rates: Rates::default(),
            modules: vec![],
            account_provisioning: AccountProvisioning {
                automatic: true,
                existing_only: false,
            },
            manager_version: crate::MANAGER_VERSION.into(),
        };
        registry
            .register(
                &key,
                &RegisterRequest {
                    protocol_version: REGISTRY_PROTOCOL_VERSION,
                    realm_id: realm,
                    public_key: coa_registry_proto::sign::encode_public_key(&key.verifying_key()),
                    listing_hash: listing.hash(),
                    listing,
                    capabilities_hash: advertised.advert_hash(),
                    capabilities: advertised,
                    population: Population {
                        players: 0,
                        bots: 0,
                        capacity: None,
                    },
                },
                clock(),
            )
            .unwrap();
        // Always unpublish this isolated identity, including after a failed assertion.
        struct Lease(RegistryClient, RealmId, ed25519_dalek::SigningKey);
        impl Drop for Lease {
            fn drop(&mut self) {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                let _ = self.0.unpublish(&self.2, &self.1, ts);
            }
        }
        let _lease = Lease(registry, realm, key.clone());
        let temporary = tempdir().unwrap();
        let settings_dir = temporary.path().join("registry");
        let local_id = "srv-live-candidate";
        let mut cfg = settings::RegistrySettings::default();
        cfg.realms.insert(
            local_id.into(),
            settings::PublishConfig {
                realm_id: Some(realm),
                enabled: true,
                display_name: "Candidate".into(),
                description: String::new(),
                language: "en".into(),
                region: None,
                existing_only: false,
                allow_portable: true,
                route: None,
            },
        );
        settings::save(&settings_dir, &cfg).unwrap();
        let installed = root.clone();
        let runtime = Arc::new(
            PortableRuntime::start(
                &temporary.path().join("portable"),
                Box::new(move || vec![("live-candidate".into(), installed.clone())]),
                None,
            )
            .unwrap(),
        );
        let installed = root.clone();
        let backend = LocalBackend::new(
            runtime.clone(),
            &settings_dir,
            temporary.path().join("descriptors"),
            Arc::new(move || vec![("live-candidate".into(), installed.clone())]),
        );
        assert_eq!(
            backend.info(local_id).unwrap().ruleset,
            crate::portable::Ruleset::Coa
        );
        let service = Arc::new(HostService::new(
            local_id,
            realm,
            Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap())),
            Arc::new(backend),
        ));
        let relay = crate::control::relay_link::RelayLink::start_with_local_ports(
            coordinator_url(&base, "/relay/v1/host").unwrap(),
            realm,
            key.clone(),
            auth_port,
            Some(57638),
        );
        service.set_relay(relay.clone());
        let mut link = HostLink::start(
            coordinator_url(&base, "/coord/v1/host").unwrap(),
            realm,
            key.clone(),
            service,
            Arc::new(clock),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while !link.status().connected && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(link.status().connected, "{:?}", link.status());
        let player_secrets = Arc::new(MemoryStore::default());
        let player = Arc::new(
            PlayerControl::open(&temporary.path().join("player"), player_secrets.clone()).unwrap(),
        );
        let target = RealmTarget {
            realm_id: realm,
            key: key.verifying_key(),
            coordinator: coordinator_url(&base, "/coord/v1/player").unwrap(),
        };
        let mut channel = player.connect(&target).unwrap();
        let account = player
            .ensure_account(&mut channel, &realm, Some("REALPORTABLEGATE"))
            .unwrap();
        let until = Instant::now() + Duration::from_secs(20);
        while !relay.status().connected && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(relay.status().connected, "{:?}", relay.status());
        let allocation = player.allocate_relay(&mut channel).unwrap();
        use std::io::{Read, Write};
        let mut auth_client =
            std::net::TcpStream::connect((allocation.relay_host.as_str(), allocation.auth_port))
                .unwrap();
        auth_client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let username = account.username.to_ascii_uppercase();
        let mut challenge = vec![0, 0];
        challenge.extend_from_slice(&((30 + username.len()) as u16).to_le_bytes());
        challenge.extend_from_slice(b"WoW\0");
        challenge.extend_from_slice(&[3, 3, 5]);
        challenge.extend_from_slice(&12340u16.to_le_bytes());
        challenge.extend_from_slice(b"68x\0niW\0SUne");
        challenge.extend_from_slice(&0u32.to_le_bytes());
        challenge.extend_from_slice(&[127, 0, 0, 1]);
        challenge.push(username.len() as u8);
        challenge.extend_from_slice(username.as_bytes());
        auth_client.write_all(&challenge).unwrap();
        let mut response = [0u8; 35];
        auth_client.read_exact(&mut response).unwrap();
        assert_eq!(
            &response[..3],
            &[0, 0, 0],
            "Real authserver must accept the relay challenge"
        );
        assert!(response[3..].iter().any(|byte| *byte != 0));
        drop(auth_client);
        let mut world_client =
            std::net::TcpStream::connect((allocation.relay_host.as_str(), allocation.world_port))
                .unwrap();
        world_client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut response = [0u8; 44];
        world_client.read_exact(&mut response).unwrap();
        assert_eq!(
            &response[2..4],
            &[0xec, 0x01],
            "Real worldserver authentication challenge must pass through relay"
        );
        drop(world_client);

        let mut owner = Store::open_in_memory().unwrap();
        let profile = owner.default_profile().unwrap();
        let character = fixture_character(&databases, &mut owner, profile);
        let mut model = owner.load_current(character).unwrap();
        model.progression.level = 50;
        model.progression.xp = 0;
        let hidden_items = model.items.len();
        let owner_path = temporary.path().join("player-portable");
        let owner_runtime =
            Arc::new(PortableRuntime::start(&owner_path, Box::new(Vec::new), None).unwrap());
        owner_runtime
            .call(move |service| {
                let profile = service.owner_profile();
                service
                    .owner_mut()
                    .host_install_copy(profile, &model, 1, "test-owner")
                    .unwrap();
            })
            .unwrap();
        let public_realm = realm.to_string();
        let (bundle, session, revision) = owner_runtime
            .call(move |service| {
                service.prepare_remote_transfer(&character.to_string(), &public_realm)
            })
            .unwrap()
            .unwrap();
        let transferred = player
            .transfer_owned_character(
                &mut channel,
                session,
                character.as_uuid(),
                "coa",
                revision,
                &bundle.payload,
                bundle.content_hash,
                BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(transferred.session_id, session);
        assert!(transferred.local_guid > 0);
        assert_eq!(
            number(
                &databases.b,
                &format!(
                    "SELECT level FROM acore_characters.characters WHERE guid = {}",
                    transferred.local_guid
                )
            ),
            50
        );
        let retry = player
            .transfer_owned_character(
                &mut channel,
                session,
                character.as_uuid(),
                "coa",
                revision,
                &bundle.payload,
                bundle.content_hash,
                BTreeMap::new(),
            )
            .unwrap();
        assert_eq!(
            (retry.local_guid, retry.session_id),
            (transferred.local_guid, transferred.session_id)
        );
        let pending = player.pending_remote_sessions().unwrap().pop().unwrap();
        let mut console = server.ra();
        console
            .run(&format!("botcmd spawnbot {}", transferred.local_guid))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        let sid = crate::portable::SessionId::from_uuid(session).unwrap();
        let mut playing = false;
        while Instant::now() < deadline {
            player
                .sync_remote_session(&pending, &owner_runtime)
                .unwrap();
            let opened = owner_runtime
                .call(move |service| {
                    service
                        .owner()
                        .owner_session_info(sid)
                        .unwrap()
                        .is_some_and(|row| row.0 == "open")
                })
                .unwrap();
            if opened
                && crate::portable::session::live_session::online(
                    &databases.b,
                    transferred.local_guid,
                )
                && console
                    .run(&format!("portable status {}", transferred.local_guid))
                    .unwrap_or_default()
                    .contains("gated 0")
            {
                playing = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(
            playing,
            "The actual player never received the Owner's durable baseline acknowledgement"
        );
        let name =
            crate::portable::session::live_session::name_of(&databases.b, transferred.local_guid);
        console.run(&format!("character level {name} 51")).unwrap();
        // Reopen both Player databases before receiving progress. Recovery uses the pinned realm key and durable session.
        owner_runtime.shutdown();
        drop(owner_runtime);
        drop(player);
        let owner_runtime =
            Arc::new(PortableRuntime::start(&owner_path, Box::new(Vec::new), None).unwrap());
        let player = Arc::new(
            PlayerControl::open(&temporary.path().join("player"), player_secrets).unwrap(),
        );
        let pending = player.pending_remote_sessions().unwrap().pop().unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut saved = false;
        while Instant::now() < deadline {
            player
                .sync_remote_session(&pending, &owner_runtime)
                .unwrap();
            saved = owner_runtime
                .call(move |service| {
                    service
                        .owner()
                        .load_current(character)
                        .unwrap()
                        .progression
                        .level
                        == 51
                })
                .unwrap();
            if saved {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(saved, "Real in-game progress was not saved through the public coordinator after Manager restart");
        assert!(crate::portable::session::live_session::online(
            &databases.b,
            transferred.local_guid
        ));
        console
            .run(&format!("botcmd despawn {}", transferred.local_guid))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut completed = false;
        while Instant::now() < deadline {
            if player
                .sync_remote_session(&pending, &owner_runtime)
                .unwrap()
            {
                completed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(
            completed,
            "The final progress acknowledgement never completed"
        );
        let next = player.pending_remote_sessions().unwrap().pop().unwrap();
        assert_ne!(
            next.session_id, session,
            "The next login must remain monitored"
        );
        assert!(owner_runtime
            .call(move |service| service.remote_session_idle(next.session_id))
            .unwrap()
            .unwrap());
        // A second login without pressing Play is still supervised and saved.
        PlayerControl::start_remote_session_sync(&player, &owner_runtime);
        console
            .run(&format!("botcmd spawnbot {}", transferred.local_guid))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        let next_sid = crate::portable::SessionId::from_uuid(next.session_id).unwrap();
        let mut opened = false;
        while Instant::now() < deadline {
            opened = owner_runtime
                .call(move |service| {
                    service
                        .owner()
                        .owner_session_info(next_sid)
                        .unwrap()
                        .is_some_and(|row| row.0 == "open")
                })
                .unwrap();
            if opened {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(
            opened
                && crate::portable::session::live_session::online(
                    &databases.b,
                    transferred.local_guid
                )
        );
        console.run(&format!("character level {name} 52")).unwrap();
        console
            .run(&format!("botcmd despawn {}", transferred.local_guid))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut completed = false;
        while Instant::now() < deadline {
            if player
                .pending_remote_sessions()
                .unwrap()
                .iter()
                .all(|pending| pending.session_id != next.session_id)
            {
                completed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(
            completed,
            "The second login's final checkpoint was not acknowledged"
        );
        player
            .leave_idle_remote_session(character.as_uuid(), &owner_runtime)
            .unwrap();
        assert!(player.pending_remote_sessions().unwrap().is_empty());
        owner_runtime
            .call(move |service| {
                assert!(service.remote_session_closed(session).unwrap());
                let current = service.owner().load_current(character).unwrap();
                assert_eq!(current.progression.level, 52);
                assert!(
                    current.items.len() >= hidden_items,
                    "Hidden equipment was lost"
                );
            })
            .unwrap();
        owner_runtime.shutdown();
        channel.close();
        link.stop();
        relay.stop();
        runtime.shutdown();
        drop(server);
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

        assert_eq!(backend.auth_port("srv-test1:coa").unwrap(), 3724);
        fs::write(
            root.join("Settings/repack.json"),
            r#"{"worldPort":8085,"authPort":57324}"#,
        )
        .unwrap();
        assert_eq!(backend.auth_port("srv-test1:coa").unwrap(), 57324);
        assert_eq!(backend.auth_port("srv-test1:wildcard").unwrap(), 57324);
        fs::write(
            root.join("Settings/repack.json"),
            r#"{"worldPort":8085,"authPort":0}"#,
        )
        .unwrap();
        assert!(backend.auth_port("srv-test1:coa").is_err());
        fs::write(
            root.join("Settings/repack.json"),
            r#"{"worldPort":8085,"authPort":57324}"#,
        )
        .unwrap();
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
