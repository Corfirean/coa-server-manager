//! Command-line harness for the portable-characters round trip, for manual smoke tests against a **disposable** realm database.
//! It performs exactly the operations the Manager performs (`coa_core::portable::realm`), one per invocation, with a Manager
//! store kept in a directory of your choice. It is not a user interface and not part of the shipped Manager.
//!
//! ```text
//! portable_smoke --tools <mysql bin dir> --port <port> --store <dir> [--user root] [--realm coa] <command> [args]
//!   the database password is read from the environment variable COA_DB_PASSWORD
//!
//! commands
//!   list                                        characters of the realm and why they cannot be exported
//!   make-portable <server-id> <guid>            export an offline character into the store (revision 1)
//!   import <character-id> <server-id> <account> create the character on a STOPPED realm
//!   begin-session <character-id> <server-id>    freeze B0: run it after the realm's first load/save, before play
//!   reconcile <character-id> <server-id> [--close]   bring what was played back into the store
//!   update <character-id> <server-id>           bring the realm's own character to the newest revision, in place
//!   recover <server-id>                         finish or abort interrupted imports and updates
//!   show <character-id>                         revisions and realm bindings
//!   session-offer <character-id> <server-id> <offer.json>   (owner store)  offer a character and open its runtime session
//!   session-import <offer.json> <account> <server-id>       (host store: --host-store)  keep the copy, import it with the session
//!                      armed (online through the core's import service when --ra-port and --job-dir are given, else offline)
//!   host-run <owner-store-dir> <server-id>                  (host store)  run the Host: automatic baseline, a checkpoint every
//!                      --interval seconds (default 60), the final checkpoint at logout; messages go straight to the owner store
//!   dump <character-id>                         the canonical JSON of the current revision (what a core import job carries)
//!   collection-states <out.json>                (owner store)  the canonical account collections as messages
//!   collection-apply <states.json> <account> <server-id>   write the canonical collections to the realm account (known ids only, INSERT IGNORE)
//!   capabilities <server-id>                    the realm's content profile (from its core when --ra-port is given), remembered in the store
//!   preflight <character-id> <server-id> [online|session]   what would happen to the character on that realm, before anything is written
//!   reevaluate <character-id> <server-id>       look again at what was held back when the realm's content profile changed (realm stopped)
//!   collection-add <kind> <id>...               (owner store)  add ids to a profile collection by hand (a test aid)
//!   collection-show [<account>]                 the Owner's collections (revision, count, hash) and, with an account, the realm's
//!   project <character-id> <server-id>          ask the realm's running core what a level-cap projection of the character holds (needs --ra-port
//!                      and --job-dir); --out <file> writes the decision, which a later `import`/`update` of a STOPPED realm can be given with
//!                      --projection <file> (it answers only for the exact state of the character it was made for)
//!
//!   --collection-interval <seconds>   how often host-run looks at the account collections (default 300; a session start and the final
//!                                 checkpoint always look)
//!   --data-dir <realm Data dir>   the destination's client data (Appearances.dbc, VanityCollection.dbc): without it no selected
//!                                 appearance and no collection is written to the realm (everything stays canonical)
//! ```

use std::path::PathBuf;

use coa_core::db::Db;
use coa_core::portable::realm::{self, ImportOptions};
use coa_core::portable::session::live::LiveBridge;
use coa_core::portable::session::protocol::SessionOffer;
use coa_core::portable::session::{HostConfig, HostService, OwnerService};
use coa_core::portable::{CharacterId, Store};
use coa_core::realms::Mode;

/// The options of a command, with the realm's content profile: assembled from its schema, its client data and (given an RA port) its
/// core, and remembered in the store. Without `--data-dir` there is no profile and nothing is evaluated.
fn with_profile(db: &Db, store: &mut Store, base: &ImportOptions, data_dir: &str, ra_port: Option<u16>, ra_user: &str, server: &str) -> Result<ImportOptions, String> {
    if data_dir.is_empty() {
        return Ok(base.clone());
    }
    let registry = coa_core::portable::extension::ExtensionRegistry::new();
    let mut ra = ra_port.and_then(|port| std::env::var("COA_RA_PASSWORD").ok().and_then(|pw| coa_core::ra::Ra::connect_to(port, ra_user, &pw).ok()));
    let caps = realm::profile::probe_capabilities(db, Some(std::path::Path::new(data_dir)), ra.as_mut(), &registry).map_err(|e| e.to_string())?;
    let change = store.set_realm_profile(server, &caps, if ra.is_some() { "live" } else { "offline" }).map_err(|e| e.to_string())?;
    eprintln!("realm {server}: content profile {}{}", &caps.content_profile_hash[..16], if change.changed() { " (new or changed)" } else { "" });
    Ok(ImportOptions { capabilities: Some(std::sync::Arc::new(caps)), extensions: Some(std::sync::Arc::new(registry)), ..base.clone() })
}

fn usage() -> ! {
    eprintln!("{}", include_str!("portable_smoke.rs").lines().take_while(|l| l.starts_with("//!")).map(|l| l.trim_start_matches("//!").trim_start_matches(' ')).collect::<Vec<_>>().join("\n"));
    std::process::exit(2)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut take = |name: &str, default: Option<&str>| -> Result<String, String> {
        if let Some(i) = args.iter().position(|a| a == name) {
            if i + 1 >= args.len() {
                return Err(format!("{name} needs a value"));
            }
            let v = args.remove(i + 1);
            args.remove(i);
            return Ok(v);
        }
        default.map(str::to_string).ok_or_else(|| format!("{name} is required"))
    };
    let tools = PathBuf::from(take("--tools", None)?);
    let port: u16 = take("--port", None)?.parse().map_err(|_| "--port is not a number".to_string())?;
    let store_dir = PathBuf::from(take("--store", None)?);
    let user = take("--user", Some("root"))?;
    let realm_mode = match take("--realm", Some("coa"))?.as_str() {
        "coa" => Mode::Coa,
        other => return Err(format!("--realm {other}: only coa is supported (Wildcard portable transfer is not supported)")),
    };
    let password = std::env::var("COA_DB_PASSWORD").map_err(|_| "set COA_DB_PASSWORD".to_string())?;
    // `with_tools` takes a 'static user name; this is a short-lived process
    let user: &'static str = Box::leak(user.into_boxed_str());
    let db = Db::with_tools(tools, port, user, &password, realm_mode);
    let mut store = Store::open(&store_dir).map_err(|e| e.to_string())?;
    let data_dir = take("--data-dir", Some(""))?;
    let knowledge = match data_dir.as_str() {
        "" => None,
        dir => Some(std::sync::Arc::new(coa_core::portable::realm::knowledge::RealmKnowledge::from_data_dir(std::path::Path::new(dir)).map_err(|e| e.to_string())?)),
    };
    let projection_file = take("--projection", Some(""))?;
    let out_file = take("--out", Some(""))?;
    let supplied = match projection_file.as_str() {
        "" => None,
        file => {
            let hold: coa_core::portable::projection::ProjectionHold = serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?).map_err(|e| format!("{file}: {e}"))?;
            Some(coa_core::portable::projection::Oracle(std::sync::Arc::new(coa_core::portable::projection::SuppliedDecision(hold))))
        }
    };
    let opts = ImportOptions { game_server_users: vec![user.to_string(), "acore".to_string()], knowledge: knowledge.clone(), projection: supplied, ..ImportOptions::default() };
    let host_dir = PathBuf::from(take("--host-store", Some(&format!("{}-host", store_dir.display())))?);
    let ra_port: Option<u16> = take("--ra-port", Some("0"))?.parse().ok().filter(|p| *p != 0);
    let ra_user = take("--ra-user", Some("local"))?;
    let job_dir = take("--job-dir", Some(""))?;
    let profile_for = |db: &Db, store: &mut Store, server: &str| -> Result<ImportOptions, String> { with_profile(db, store, &opts, &data_dir, ra_port, &ra_user, server) };
    let interval: u64 = take("--interval", Some("60"))?.parse().map_err(|_| "--interval is not a number".to_string())?;
    let collection_interval: u64 = take("--collection-interval", Some("300"))?.parse().map_err(|_| "--collection-interval is not a number".to_string())?;
    let id = |s: &str| -> Result<CharacterId, String> { s.parse().map_err(|_| format!("{s:?} is not a character id")) };

    let command = if args.is_empty() { usage() } else { args.remove(0) };
    match (command.as_str(), args.as_slice()) {
        ("list", []) => {
            for c in realm::inspect_characters(&db).map_err(|e| e.to_string())? {
                println!("{}\t{}\tlevel {}\trace {} class {}\t{}", c.local_guid, c.name, c.level, c.race, c.class, if c.eligible() { "ok".to_string() } else { c.blockers.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("; ") });
            }
        }
        ("make-portable", [server, guid]) => {
            let profile = store.default_profile().map_err(|e| e.to_string())?;
            let made = realm::make_portable(&db, &mut store, profile, server, guid.parse().map_err(|_| "guid")?).map_err(|e| e.to_string())?;
            println!("portable character {} (revision {}) {}", made.character_id, made.revision, made.warnings.join("; "));
        }
        ("import", [character, server, account]) => {
            let opts = profile_for(&db, &mut store, server)?;
            let o = realm::import_character(&db, &mut store, id(character)?, server, account.parse().map_err(|_| "account")?, &opts).map_err(|e| e.to_string())?;
            for line in &o.not_applied {
                println!("  not applied: {line}");
            }
            println!("imported as local character {} \"{}\" ({} items, {} pets, renamed: {})", o.local_guid, o.final_name, o.items, o.pets, o.renamed);
        }
        ("begin-session", [character, server]) => {
            let s = realm::begin_session(&db, &mut store, id(character)?, server).map_err(|e| e.to_string())?;
            println!("baseline captured at canonical revision {}: {} item(s) held back by the realm, {} of its own", s.c0_revision, s.items_filtered, s.items_realm_local);
        }
        ("reconcile", [character, server, rest @ ..]) => {
            let close = rest.iter().any(|a| a == "--close");
            let o = realm::reconcile_session(&db, &mut store, id(character)?, server, close, Some("portable_smoke")).map_err(|e| e.to_string())?;
            println!("canonical revision {}{}", o.revision, if o.new_revision { " (new)" } else { " (unchanged)" });
            for c in o.changes {
                println!("  {c}");
            }
        }
        ("update", [character, server]) => {
            let opts = profile_for(&db, &mut store, server)?;
            let o = realm::update_realm_character(&db, &mut store, id(character)?, server, &opts).map_err(|e| e.to_string())?;
            for line in &o.warnings {
                println!("  {line}");
            }
            println!("{} (revision {} -> {}): {:?}", if o.updated { "updated in place" } else { "already up to date" }, o.from_revision, o.to_revision, o.counts);
        }
        ("recover", [server]) => {
            for r in realm::recover_imports(&db, &mut store, server, &opts).map_err(|e| e.to_string())? {
                println!("{}: {:?}", r.import_id, r.resolution);
            }
        }
        ("session-offer", [character, server, out]) => {
            let offer = OwnerService::new(&mut store).offer(id(character)?, server).map_err(|e| e.to_string())?;
            std::fs::write(out, serde_json::to_vec_pretty(&offer).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            println!("session {} offered at canonical revision {} -> {out}", offer.session_id, offer.canonical_revision);
        }
        ("session-import", [file, account, server]) => {
            let offer: SessionOffer = serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let mut host = Store::open(&host_dir).map_err(|e| e.to_string())?;
            let profile = host.default_profile().map_err(|e| e.to_string())?;
            HostService::new(&mut host, server, HostConfig::default()).accept_offer(profile, &offer).map_err(|e| e.to_string())?;
            let account: u32 = account.parse().map_err(|_| "account")?;
            let opts = with_profile(&db, &mut host, &opts, &data_dir, ra_port, &ra_user, server)?;
            let outcome = match (ra_port, job_dir.is_empty()) {
                (Some(port), false) => {
                    let mut ra = coa_core::ra::Ra::connect_to(port, &ra_user, &std::env::var("COA_RA_PASSWORD").map_err(|_| "set COA_RA_PASSWORD".to_string())?).map_err(|e| e.to_string())?;
                    realm::online::import_character_online(&mut ra, &mut host, offer.character_id, server, account, &opts, std::path::Path::new(&job_dir), Some(offer.session_id)).map_err(|e| e.to_string())?
                }
                _ => realm::import_character_in_session(&db, &mut host, offer.character_id, server, account, &opts, Some(offer.session_id)).map_err(|e| e.to_string())?,
            };
            HostService::new(&mut host, server, HostConfig::default()).bind(offer.session_id, outcome.local_guid).map_err(|e| e.to_string())?;
            println!("imported as local character {} \"{}\"; session {} is armed: the realm takes the baseline at its first load", outcome.local_guid, outcome.final_name, offer.session_id);
        }
        ("host-run", [owner_dir, server]) => {
            let mut owner = Store::open(&PathBuf::from(owner_dir)).map_err(|e| e.to_string())?;
            let mut host = Store::open(&host_dir).map_err(|e| e.to_string())?;
            let mut service = HostService::new(&mut host, server, HostConfig { checkpoint_interval_secs: interval, collection_interval_secs: collection_interval });
            let start = std::time::Instant::now();
            println!("host running for {server}; Ctrl-C to stop");
            loop {
                let ra = ra_port.and_then(|port| std::env::var("COA_RA_PASSWORD").ok().and_then(|pw| coa_core::ra::Ra::connect_to(port, &ra_user, &pw).ok()));
                let mut bridge = match ra {
                    Some(ra) => LiveBridge::new(&db, ra),
                    None => LiveBridge::without_console(&db),
                }
                .with_knowledge(knowledge.clone());
                match service.tick(&mut bridge, start.elapsed().as_secs()) {
                    Ok(events) => events.iter().for_each(|e| println!("{e:?}")),
                    Err(e) => eprintln!("tick failed: {e}"),
                }
                for m in service.outbox().map_err(|e| e.to_string())? {
                    let ack = if m.started { OwnerService::new(&mut owner).handle_started(&m.bytes) } else { OwnerService::new(&mut owner).handle_checkpoint(&m.bytes) };
                    match ack {
                        Ok(ack) => {
                            println!("owner: session {} #{}: {:?} (canonical revision {})", ack.session_id, ack.sequence, ack.outcome, ack.canonical_revision);
                            if let Err(e) = service.receive_ack(&mut bridge, &ack) {
                                eprintln!("acknowledgement failed: {e}");
                            }
                        }
                        Err(e) => eprintln!("owner refused a message: {e}"),
                    }
                }
                for m in service.collection_outbox().map_err(|e| e.to_string())? {
                    match OwnerService::new(&mut owner).handle_collection(&m.bytes) {
                        Ok(ack) => {
                            println!("owner: {} of account {}: {:?} (collection revision {}, canonical sent back: {})", ack.kind, m.account, ack.outcome, ack.collection_revision, ack.canonical.is_some());
                            match service.receive_collection_ack(&mut bridge, m.account, &ack) {
                                Ok(Some(applied)) => println!("host: applied to the realm: {applied:?}"),
                                Ok(None) => {}
                                Err(e) => eprintln!("collection acknowledgement failed: {e}"),
                            }
                        }
                        Err(e) => eprintln!("owner refused a collection message: {e}"),
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        }
        ("collection-states", [out]) => {
            let states = OwnerService::new(&mut store).collection_states().map_err(|e| e.to_string())?;
            for s in &states {
                println!("{}: revision {}, {} ids", s.kind, s.collection_revision, s.set.count);
            }
            std::fs::write(out, serde_json::to_vec(&states).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        ("collection-apply", [file, account, server]) => {
            let states: Vec<coa_core::portable::session::protocol::CollectionState> = serde_json::from_slice(&std::fs::read(file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let account: u32 = account.parse().map_err(|_| "account")?;
            let mut host = Store::open(&host_dir).map_err(|e| e.to_string())?;
            let mut bridge = LiveBridge::without_console(&db).with_knowledge(knowledge.clone());
            let mut service = HostService::new(&mut host, server, HostConfig::default());
            for state in &states {
                let applied = service.receive_collection_state(&mut bridge, account, state).map_err(|e| e.to_string())?;
                println!("{}: {:?}", state.kind, applied);
            }
        }
        ("capabilities", [server]) => {
            let caps = profile_for(&db, &mut store, server)?.capabilities.ok_or("give --data-dir to read the realm's client data")?;
            println!("{}", serde_json::to_string_pretty(&*caps).map_err(|e| e.to_string())?);
        }
        ("preflight", [character, server, rest @ ..]) => {
            let opts = profile_for(&db, &mut store, server)?;
            let caps = opts.capabilities.clone().ok_or("give --data-dir to read the realm's client data")?;
            let model = store.load_current(id(character)?).map_err(|e| e.to_string())?;
            let operation = match rest.first().map(String::as_str) {
                Some("online") => coa_core::portable::compat::Operation::OnlineImport,
                Some("session") => coa_core::portable::compat::Operation::RuntimeSession,
                _ if store.server_mappings(model.character_id).map_err(|e| e.to_string())?.iter().any(|m| &m.server_id == server) => coa_core::portable::compat::Operation::Update,
                _ => coa_core::portable::compat::Operation::OfflineImport,
            };
            let report = coa_core::portable::compat::evaluate(&coa_core::portable::compat::Inputs { operation, model: &model, capabilities: &caps, knowledge: opts.knowledge.as_deref(), collections: &[], extensions: opts.extensions.as_deref(), projection_decider: opts.projection.is_some() || matches!(operation, coa_core::portable::compat::Operation::OnlineImport) });
            println!("{} on {server} (content profile {}): {:?}", report.operation, &caps.content_profile_hash[..16], report.verdict());
            for o in &report.outcomes {
                println!("  {o}");
            }
        }
        ("project", [character, server]) => {
            let port = ra_port.ok_or("give --ra-port: only a running core can say what a projection holds")?;
            if job_dir.is_empty() {
                return Err("give --job-dir (the core's PortableImport.JobDir)".into());
            }
            let opts = profile_for(&db, &mut store, server)?;
            let canonical = store.load_current(id(character)?).map_err(|e| e.to_string())?;
            let mut ra = coa_core::ra::Ra::connect_to(port, &ra_user, &std::env::var("COA_RA_PASSWORD").map_err(|_| "set COA_RA_PASSWORD".to_string())?).map_err(|e| e.to_string())?;
            let progression = opts.capabilities.as_ref().and_then(|c| c.progression.clone());
            match realm::project::decide_with_core(&mut ra, std::path::Path::new(&job_dir), &canonical).map_err(|e| e.to_string())? {
                coa_core::portable::projection::Decision::Native => println!("not projected: level {} is within the realm's cap {}", canonical.progression.level, progression.map_or("?".to_string(), |p| p.max_player_level.to_string())),
                coa_core::portable::projection::Decision::Projected(hold) => {
                    println!("projected level {} -> {} (signature {})", hold.canonical_level, hold.projected_level, &hold.progression_signature[..16]);
                    println!("  held: {} item(s), {} ability(ies), {} button(s), {} build record(s) edited, {} blocked", hold.held_items.len(), hold.held_spells.len(), hold.held_actions.len(), hold.settings.iter().filter(|s| !s.entries.is_empty() || !s.buttons.is_empty()).count(), hold.blocked_settings.len());
                    if !out_file.is_empty() {
                        std::fs::write(&out_file, serde_json::to_vec_pretty(&hold).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                        println!("  decision written to {out_file}");
                    }
                }
            }
        }
        ("reevaluate", [character, server]) => {
            let opts = profile_for(&db, &mut store, server)?;
            let r = realm::reevaluate_realm_character(&db, &mut store, id(character)?, server, &opts).map_err(|e| e.to_string())?;
            if r.profile_unchanged {
                println!("the realm's content profile is the one this character was synchronised under: nothing to look at");
            } else {
                println!("{}", r.update.map(|u| format!("restored: {:?}", u.changes)).unwrap_or_else(|| "nothing new to show on this realm".into()));
                for e in r.extensions {
                    println!("  {e}");
                }
            }
        }
        ("collection-add", [kind, ids @ ..]) => {
            let profile = store.default_profile().map_err(|e| e.to_string())?;
            let ids = ids.iter().map(|i| i.parse::<u32>().map_err(|_| format!("{i:?} is not an id"))).collect::<Result<Vec<_>, _>>()?;
            let set = coa_core::portable::collection::IdSet::from_ids(ids).map_err(|e| e.to_string())?;
            let merge = store.merge_collection(profile, kind, &set).map_err(|e| e.to_string())?;
            println!("{kind}: {} id(s) added{}", merge.added, merge.info.map(|i| format!(", revision {}, {} ids", i.revision, i.count)).unwrap_or_default());
        }
        ("collection-show", rest) => {
            let profile = store.default_profile().map_err(|e| e.to_string())?;
            for kind in coa_core::portable::session::protocol::COLLECTION_KINDS {
                match store.collection_info(profile, kind).map_err(|e| e.to_string())? {
                    Some(i) => println!("owner {kind}: revision {}, {} ids, hash {}", i.revision, i.count, hex::encode(&i.hash[..8])),
                    None => println!("owner {kind}: none"),
                }
                if let [account] = rest {
                    let account: u32 = account.parse().map_err(|_| "account")?;
                    let set = coa_core::portable::realm::collections::read_set(&db, account, kind).map_err(|e| e.to_string())?;
                    println!("realm {kind} of account {account}: {} ids", set.len());
                }
            }
        }
        ("dump", [character]) => {
            let model = store.load_current(id(character)?).map_err(|e| e.to_string())?;
            let json = coa_core::portable::snapshot::canonical_json(&model).map_err(|e| e.to_string())?;
            println!("{}", String::from_utf8_lossy(&json));
        }
        ("show", [character]) => {
            let c = id(character)?;
            let r = store.character(c).map_err(|e| e.to_string())?;
            println!("{} {} level {} revision {}", r.character_id, r.name, r.level, r.revision);
            for m in store.server_mappings(c).map_err(|e| e.to_string())? {
                println!("  on {}: local guid {} at revision {} ({:?})", m.server_id, m.local_guid, m.last_revision, m.state);
            }
        }
        _ => usage(),
    }
    Ok(())
}
