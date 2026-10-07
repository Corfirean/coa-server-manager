//! The control plane end to end: a Host Manager (HostLink + HostService over a fake realm), a Player Manager (PlayerControl), and the real Coordinator between them,
//! with a recording tap on the wire to prove that what the Coordinator carries is not readable.

use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coa_coordinator::listener::{LimitedListener, Peer};
use coa_coordinator::{router, Config, Hub, MemoryKeys};
use coa_core::control::host_link::HostLink;
use coa_core::control::identity::PlayerIdentity;
use coa_core::control::join::{PlayerControl, RealmTarget};
use coa_core::control::player::PlayerChannel;
use coa_core::control::secrets::{FileStore, MemoryStore, SecretStore};
use coa_core::control::service::fake::FakeRealm;
use coa_core::control::service::{HostService, RealmBackend};
use coa_core::control::store::ControlStore;
use coa_registry_proto::RealmId;
use ed25519_dalek::SigningKey;
use tungstenite::Message;
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

/// Forwards WebSocket messages between a client and the upstream, recording every payload that crosses.
fn tap(upstream: SocketAddr) -> (SocketAddr, Arc<Mutex<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let record = record.clone();
            std::thread::spawn(move || {
                let mut path = String::new();
                let mut client = match tungstenite::accept_hdr(stream, |req: &tungstenite::handshake::server::Request, resp| {
                    path = req.uri().path_and_query().map(|p| p.to_string()).unwrap_or_default();
                    Ok(resp)
                }) {
                    Ok(c) => c,
                    Err(_) => return,
                };
                let Ok((mut up, _)) = tungstenite::connect(format!("ws://{upstream}{path}")) else { return };
                let set = |ws: &mut tungstenite::WebSocket<_>| {
                    let tcp: &std::net::TcpStream = match ws.get_mut() {
                        tungstenite::stream::MaybeTlsStream::Plain(t) => t,
                        _ => return,
                    };
                    let _ = tcp.set_read_timeout(Some(Duration::from_millis(5)));
                };
                let _ = client.get_mut().set_read_timeout(Some(Duration::from_millis(5)));
                set(&mut up);
                let pump = |from: &mut dyn FnMut() -> Result<Option<Message>, ()>, to: &mut dyn FnMut(Message) -> Result<(), ()>, record: &Mutex<Vec<u8>>| -> Result<(), ()> {
                    if let Some(m) = from()? {
                        match &m {
                            Message::Text(t) => record.lock().unwrap().extend_from_slice(t.as_bytes()),
                            Message::Binary(b) => record.lock().unwrap().extend_from_slice(b),
                            _ => {}
                        }
                        if matches!(m, Message::Text(_) | Message::Binary(_) | Message::Close(_)) {
                            to(m)?;
                        }
                    }
                    Ok(())
                };
                loop {
                    let a = pump(
                        &mut || match client.read() {
                            Ok(m) => Ok(Some(m)),
                            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
                            Err(_) => Err(()),
                        },
                        &mut |m| up.send(m).map_err(|_| ()),
                        &record,
                    );
                    if a.is_err() {
                        return;
                    }
                    let b = pump(
                        &mut || match up.read() {
                            Ok(m) => Ok(Some(m)),
                            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
                            Err(_) => Err(()),
                        },
                        &mut |m| client.send(m).map_err(|_| ()),
                        &record,
                    );
                    if b.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, seen)
}

struct Env {
    realm: RealmId,
    realm_key: ed25519_dalek::VerifyingKey,
    fake: Arc<FakeRealm>,
    link: Option<HostLink>,
    store: Arc<Mutex<ControlStore>>,
    tap_addr: SocketAddr,
    seen: Arc<Mutex<Vec<u8>>>,
}

impl Env {
    fn new(automatic: bool) -> Env {
        let keys = Arc::new(MemoryKeys::default());
        let hub = Hub::new(keys.clone(), Config { per_ip_connections: 64, per_ip: coa_coordinator::limits::Rate::per_minute(1000, 1000.0), per_realm: coa_coordinator::limits::Rate::per_minute(1000, 1000.0), ..Config::default() }, Arc::new(now));
        let app = router(hub);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            rt.block_on(async move {
                let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(tcp.local_addr().unwrap()).unwrap();
                axum::serve(LimitedListener::new(tcp, 64), app.into_make_service_with_connect_info::<Peer>()).await.unwrap();
            });
        });
        let coordinator = rx.recv().unwrap();
        let (tap_addr, seen) = tap(coordinator);
        let realm = RealmId::new();
        let key = random_key();
        keys.set(realm, &key.verifying_key());
        let fake = FakeRealm::new(automatic);
        let store = Arc::new(Mutex::new(ControlStore::open_in_memory().unwrap()));
        let service = Arc::new(HostService::new("srv-1", realm, store.clone(), fake.clone()));
        let link = HostLink::start(format!("ws://{tap_addr}/coord/v1/host"), realm, key.clone(), service, Arc::new(now));
        let end = Instant::now() + Duration::from_secs(10);
        while !link.status().connected {
            assert!(Instant::now() < end, "the host never connected: {:?}", link.status());
            std::thread::sleep(Duration::from_millis(50));
        }
        Env { realm, realm_key: key.verifying_key(), fake, link: Some(link), store, tap_addr, seen }
    }

    fn target(&self) -> RealmTarget {
        RealmTarget { realm_id: self.realm, key: self.realm_key, coordinator: format!("ws://{}/coord/v1/player", self.tap_addr) }
    }

    fn wire_contains(&self, needle: &str) -> bool {
        let seen = self.seen.lock().unwrap();
        !seen.is_empty() && seen.windows(needle.len()).any(|w| w == needle.as_bytes())
    }
}

fn player(dir: &std::path::Path, secrets: Arc<dyn SecretStore>) -> PlayerControl {
    PlayerControl::open(dir, secrets).unwrap()
}

#[test]
fn a_new_player_gets_an_account_and_keeps_it() {
    let env = Env::new(true);
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    let mut ch = pc.connect(&env.target()).unwrap();
    assert_eq!(pc.welcome(&mut ch).unwrap(), (true, false, None));
    let first = pc.ensure_account(&mut ch, &env.realm, Some("Dmitry")).unwrap();
    assert_eq!((first.username.as_str(), first.created, first.password_reset), ("DMITRY", true, false));
    let creds = pc.credentials(&env.realm).unwrap().unwrap();
    assert_eq!(creds.username, "DMITRY");
    assert_eq!(creds.password.len(), 16);
    assert!(env.fake.check_login("", "DMITRY", &creds.password).unwrap().is_some(), "the realm accepts the saved password");
    assert_eq!(format!("{creds:?}"), "Credentials(DMITRY, ****)", "a password is never printed");
    ch.close();

    // the next visit reuses it and changes nothing
    let mut ch = pc.connect(&env.target()).unwrap();
    let again = pc.ensure_account(&mut ch, &env.realm, Some("Dmitry")).unwrap();
    assert_eq!((again.created, again.password_reset), (false, false));
    assert_eq!(env.fake.accounts.lock().unwrap().len(), 1);
    assert_eq!(pc.credentials(&env.realm).unwrap().unwrap(), creds);

    // what crossed the Coordinator is unreadable
    for secret in [creds.password.as_str(), "DMITRY", "Dmitry", "Provision", "provision", "password"] {
        assert!(!env.wire_contains(secret), "the wire carries {secret:?}");
    }
    assert!(env.seen.lock().unwrap().len() > 500, "something did cross");
}

#[test]
fn an_occupied_name_is_replaced_by_a_stable_alternative() {
    let env = Env::new(true);
    env.fake.add_account("Dmitry", "Whatever9");
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    let mut ch = pc.connect(&env.target()).unwrap();
    let out = pc.ensure_account(&mut ch, &env.realm, Some("Dmitry")).unwrap();
    assert!(out.created && out.username.starts_with("DMITRY_") && out.username.len() == 11, "{out:?}");
    assert_eq!(pc.known_account(&env.realm).unwrap().unwrap().0, out.username);
}

#[test]
fn a_lost_password_is_replaced_without_a_second_account() {
    let env = Env::new(true);
    let dir = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
    let pc = player(dir.path(), secrets.clone());
    let mut ch = pc.connect(&env.target()).unwrap();
    pc.ensure_account(&mut ch, &env.realm, Some("Anna")).unwrap();
    let old = pc.credentials(&env.realm).unwrap().unwrap();
    ch.close();
    pc.forget(&env.realm).unwrap();
    let mut ch = pc.connect(&env.target()).unwrap();
    let out = pc.ensure_account(&mut ch, &env.realm, Some("Anna")).unwrap();
    assert!(!out.created && out.password_reset);
    let fresh = pc.credentials(&env.realm).unwrap().unwrap();
    assert_ne!(fresh.password, old.password);
    assert!(env.fake.check_login("", "ANNA", &fresh.password).unwrap().is_some() && env.fake.check_login("", "ANNA", &old.password).unwrap().is_none());
    assert_eq!(env.fake.accounts.lock().unwrap().len(), 1);
}

#[test]
fn existing_only_realms_ask_for_a_one_time_link() {
    let env = Env::new(false);
    env.fake.add_account("Legacy", "Secret12");
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    let mut ch = pc.connect(&env.target()).unwrap();
    assert_eq!(pc.welcome(&mut ch).unwrap(), (false, true, None));
    let err = pc.ensure_account(&mut ch, &env.realm, Some("Legacy")).unwrap_err();
    assert_eq!(err.code(), "provisioning_off");
    assert!(pc.credentials(&env.realm).unwrap().is_none(), "nothing pending is left behind");
    let wrong = pc.link_existing(&mut ch, &env.realm, "LEGACY", "WrongPass1").unwrap_err();
    assert_eq!(wrong.code(), "wrong_credentials");
    assert_eq!(pc.link_existing(&mut ch, &env.realm, "legacy", "Secret12").unwrap(), "LEGACY");
    assert_eq!(pc.credentials(&env.realm).unwrap().unwrap().password, "Secret12");
    // joining afterwards confirms the same account and keeps it a linked one
    let again = pc.ensure_account(&mut ch, &env.realm, None).unwrap();
    assert_eq!((again.username.as_str(), again.created, again.password_reset), ("LEGACY", false, false));
    assert_eq!(pc.known_account(&env.realm).unwrap().unwrap().1, coa_core::control::store::AccountKind::Linked);
    assert!(!env.wire_contains("Secret12") && !env.wire_contains("LEGACY"), "the link travelled encrypted");
    // linked once: the host holds the mapping, not the password
    let m = env.store.lock().unwrap().host_player("srv-1", &pc.identity.player_id).unwrap().unwrap();
    assert_eq!((m.username.as_str(), m.kind), ("LEGACY", coa_core::control::store::AccountKind::Linked));
}

#[test]
fn a_legacy_character_is_claimed_once_by_its_owner_only() {
    let env = Env::new(true);
    let acc = env.fake.add_account("Thrall", "Secret12");
    env.fake.add_char(acc, 100, "Thrall", true);
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    let mut ch = pc.connect(&env.target()).unwrap();
    assert_eq!(pc.list_characters(&mut ch).unwrap_err().code(), "no_account");
    pc.link_existing(&mut ch, &env.realm, "THRALL", "Secret12").unwrap();
    let list = pc.list_characters(&mut ch).unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].eligible && !list[0].yours && list[0].token < 100);
    // not at the character-select screen yet
    assert_eq!(pc.claim(&mut ch, list[0].token).unwrap_err().code(), "not_at_character_select");
    env.fake.at_select(acc, true, 0);
    let claimed = pc.claim(&mut ch, list[0].token).unwrap();
    assert_eq!(claimed.payload, b"payload-100");
    assert_eq!(claimed.collections["coa:appearance"], vec![1, 2, 3]);
    pc.acknowledge(&mut ch, claimed.character_id, &env.realm, "Thrall").unwrap();
    assert_eq!(pc.claimed_here(&env.realm).unwrap().len(), 1);
    ch.close();
    assert!(!env.wire_contains("payload-100") && !env.wire_contains("Thrall"), "the character crossed encrypted");

    // another player (another identity) cannot take it, even holding the same account's name and password: the account is bound
    let dir2 = tempfile::tempdir().unwrap();
    let other = player(dir2.path(), Arc::new(MemoryStore::default()));
    assert_ne!(other.identity.player_id, pc.identity.player_id);
    let mut ch2 = other.connect(&env.target()).unwrap();
    let err = other.link_existing(&mut ch2, &env.realm, "THRALL", "Secret12").unwrap_err();
    assert_eq!(err.code(), "account_taken");
    assert_eq!(other.list_characters(&mut ch2).unwrap_err().code(), "no_account");
}

#[test]
fn an_impostor_host_is_refused_before_anything_private_is_sent() {
    let env = Env::new(true);
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    // the player believes the realm has a different key than the one the Host holds
    let wrong = RealmTarget { realm_id: env.realm, key: random_key().verifying_key(), coordinator: env.target().coordinator };
    let err = pc.connect(&wrong).err().expect("must refuse");
    assert_eq!(err.code(), "host_not_verified", "{err}");
    assert!(pc.credentials(&env.realm).unwrap().is_none());
    assert!(env.fake.seen_passwords.lock().unwrap().is_empty());
}

#[test]
fn an_offline_host_is_reported_as_such() {
    let mut env = Env::new(true);
    env.link.take().unwrap().stop();
    std::thread::sleep(Duration::from_millis(800));
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    let err = pc.connect(&env.target()).err().expect("no host");
    assert_eq!(err.code(), "host_offline", "{err}");
    let unknown = RealmTarget { realm_id: RealmId::new(), ..env.target() };
    assert_eq!(pc.connect(&unknown).err().unwrap().code(), "host_offline");
}

#[test]
fn a_player_id_cannot_be_used_with_another_key() {
    let env = Env::new(true);
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    let mut ch = pc.connect(&env.target()).unwrap();
    pc.ensure_account(&mut ch, &env.realm, Some("Boris")).unwrap();
    ch.close();
    // somebody who learned the player id but not the key
    let thief = PlayerIdentity { player_id: pc.identity.player_id, key: random_key() };
    let clock = || now();
    let attempt = PlayerChannel::connect(&env.target().coordinator, &env.realm, &env.realm_key, &thief, &clock).and_then(|mut c| c.request(&coa_control_proto::app::Request::ListCharacters));
    assert!(attempt.is_err(), "the host closes a channel whose key does not match the player id: {attempt:?}");
    // and the real player is unaffected
    let mut ch = pc.connect(&env.target()).unwrap();
    assert_eq!(pc.list_characters(&mut ch).unwrap().len(), 0);
}

#[test]
fn identity_accounts_and_credentials_survive_a_restart() {
    let env = Env::new(true);
    let dir = tempfile::tempdir().unwrap();
    let secrets_dir = dir.path().join("secrets");
    let (id, creds) = {
        let pc = player(dir.path(), Arc::new(FileStore::new(&secrets_dir)));
        let mut ch = pc.connect(&env.target()).unwrap();
        pc.ensure_account(&mut ch, &env.realm, Some("Carol")).unwrap();
        (pc.identity.player_id, pc.credentials(&env.realm).unwrap().unwrap())
    };
    let pc = player(dir.path(), Arc::new(FileStore::new(&secrets_dir)));
    assert_eq!(pc.identity.player_id, id);
    assert_eq!(pc.credentials(&env.realm).unwrap().unwrap(), creds);
    assert_eq!(pc.known_account(&env.realm).unwrap().unwrap().0, "CAROL");
    let mut ch = pc.connect(&env.target()).unwrap();
    let again = pc.ensure_account(&mut ch, &env.realm, Some("Carol")).unwrap();
    assert!(!again.created, "the same player on the same realm");
    // no password in the control database
    let db = std::fs::read(dir.path().join("control.sqlite")).unwrap();
    assert!(!db.windows(creds.password.len()).any(|w| w == creds.password.as_bytes()));
}

#[test]
fn a_realms_key_is_pinned_at_the_first_visit() {
    let env = Env::new(true);
    let dir = tempfile::tempdir().unwrap();
    let pc = player(dir.path(), Arc::new(MemoryStore::default()));
    pc.connect(&env.target()).unwrap().close();
    // a directory (the Registry, or anything on the way to it) that later names another key for the same realm id is refused before anything is sent
    let swapped = RealmTarget { key: random_key().verifying_key(), ..env.target() };
    let err = pc.connect(&swapped).err().expect("the pinned key differs");
    assert_eq!(err.code(), "host_not_verified", "{err}");
    // the right key still works
    pc.connect(&env.target()).unwrap().close();
}
