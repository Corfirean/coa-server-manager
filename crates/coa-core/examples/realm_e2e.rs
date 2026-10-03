//! Native smoke checks for the disposable fixture created by tools/make-fixture.ps1.
//! Never point these probes at an owned server.
use std::path::Path;

use coa_core::db::{Account, Db};
use coa_core::driver::{self, Verb};
use coa_core::realms::{self, Mode};
use coa_core::{Error, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let root = Path::new("C:/games/coa-wildcard-fixture");
    let action = args.get(1).map(String::as_str).unwrap_or("view");
    match action {
        "select" => {
            let mode = if args.get(2).map(String::as_str) == Some("wildcard") { Mode::Wildcard } else { Mode::Coa };
            println!("{}", serde_json::to_string(&realms::select(root, mode)?)?);
        }
        "start" | "stop" => {
            let out = driver::run(root, if action == "start" { Verb::StartAll } else { Verb::StopAll })?;
            println!("{}", serde_json::to_string(&out)?);
            if !out.ok { return Err(Error::Invalid("Fixture operation failed".into())); }
        }
        "probe" => {
            let db = Db::from_repack(root, Account::Admin)?;
            let s = realms::state(root)?;
            println!("{}", db.query("SELECT * FROM acore_characters.manager_profile_probe;")?);
            println!("active={} chars={} world={}", s.active.name(), s.active.schema("characters")?, s.active.schema("world")?);
        }
        "mark" => {
            Db::from_repack(root, Account::Admin)?.query("INSERT INTO acore_characters.manager_profile_probe VALUES (22);")?;
        }
        "backup" => {
            use coa_core::backup::{self, Kind, Trigger};
            let meta = root.join("fixture-manager");
            let point = backup::create(root, &meta, Kind::Full, Trigger::Manual, Some("realm isolation check".into()), &|step| println!("{step}"))?;
            assert_eq!(point.realm, Mode::Wildcard);
            for name in ["characters", "world", "auth", "coa-characters", "coa-world", "configs"] {
                assert!(point.components.iter().any(|c| c.name == name), "Missing {name}");
            }
            assert!(backup::verify(&meta, &point.id)?.ok);
            println!("Verified both realm backups: {}", point.id);
        }
        "database-checks" => {
            use coa_core::backup::{self, Kind, Trigger};
            use coa_core::manifest::{Manifest, Migration};
            use coa_core::update::{Env, RepackEnv};
            let meta = root.join("fixture-manager");
            let env = RepackEnv { root, meta_dir: &meta };
            env.ensure_stopped()?;
            let staged = root.join("fixture-migrations");
            let mut manifest: Manifest = serde_json::from_value(serde_json::json!({
                "schema": 1, "kind": "update", "version": "0.5.0", "core": {"commit": null},
                "builtAt": "fixture", "minManagerVersion": "0.5.0"
            }))?;
            for kind in ["characters", "world", "auth"] {
                let sql = format!("CREATE TABLE acore_{kind}.manager_migration_probe (value INT); INSERT INTO acore_{kind}.manager_migration_probe VALUES (42);");
                let path = staged.join(format!("{kind}/manager_realm_fixture.sql"));
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(&path, sql)?;
                manifest.migrations.push(Migration { id: "manager_realm_fixture".into(), db: kind.into(), sha256: coa_core::fsx::sha256_file(&path)?, destructive: false });
            }
            let result = env.migrate(&manifest, &staged)?;
            assert!(result.failed.is_none());
            backup::with_database(root, |db| {
                for mode in [Mode::Coa, Mode::Wildcard] {
                    let realm = db.clone().for_realm(mode);
                    for kind in ["characters", "world", "auth"] {
                        assert_eq!(realm.query(&format!("SELECT COUNT(*) FROM acore_{kind}.manager_migration_probe;"))?, "1");
                    }
                }
                Ok(())
            })?;
            assert!(env.migrate(&manifest, &staged)?.applied.is_empty());
            let point = backup::create(root, &meta, Kind::Quick, Trigger::Manual, None, &|_| {})?;
            backup::with_database(root, |db| {
                db.query("INSERT INTO acore_characters.manager_profile_probe VALUES (22);")?;
                Ok(())
            })?;
            backup::restore_database(root, &meta, &point.id, "characters")?;
            backup::with_database(root, |db| {
                assert_eq!(db.query("SELECT COUNT(*) FROM acore_characters.manager_profile_probe;")?, "0");
                assert_eq!(db.clone().for_realm(Mode::Coa).query("SELECT value FROM acore_characters.manager_profile_probe;")?, "11");
                Ok(())
            })?;
            println!("Both realms migrated once; shared auth migrated once; Wildcard restore left CoA intact.");
        }
        "view" => println!("{}", serde_json::to_string(&realms::view(root)?)?),
        _ => return Err(Error::Invalid("Unknown fixture action".into())),
    }
    Ok(())
}
