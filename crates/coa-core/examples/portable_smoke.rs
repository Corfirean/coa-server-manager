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
//! ```

use std::path::PathBuf;

use coa_core::db::Db;
use coa_core::portable::realm::{self, ImportOptions};
use coa_core::portable::{CharacterId, Store};
use coa_core::realms::Mode;

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
    let opts = ImportOptions { game_server_users: vec![user.to_string(), "acore".to_string()], ..ImportOptions::default() };
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
            let o = realm::import_character(&db, &mut store, id(character)?, server, account.parse().map_err(|_| "account")?, &opts).map_err(|e| e.to_string())?;
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
            let o = realm::update_realm_character(&db, &mut store, id(character)?, server, &opts).map_err(|e| e.to_string())?;
            println!("{} (revision {} -> {}): {:?}", if o.updated { "updated in place" } else { "already up to date" }, o.from_revision, o.to_revision, o.counts);
        }
        ("recover", [server]) => {
            for r in realm::recover_imports(&db, &mut store, server, &opts).map_err(|e| e.to_string())? {
                println!("{}: {:?}", r.import_id, r.resolution);
            }
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
