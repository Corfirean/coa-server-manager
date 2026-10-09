//! Probe only a disposable fixture: enable both worlds, verify isolation, gracefully stop both.
use coa_core::{
    driver::{self, Verb},
    multiworld,
    process::{self, ServiceState},
    realms::{self, Mode},
    Error, Result,
};
use std::path::PathBuf;

fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| Error::Invalid("fixture path required".into()))?,
    );
    if !root
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains("fixture"))
    {
        return Err(Error::Invalid("Refusing a non-fixture server.".into()));
    }
    let action = std::env::args().nth(2).unwrap_or_else(|| "smoke".into());
    if action == "stop" {
        driver::run(&root, Verb::StopAll)?;
        return Ok(());
    }
    driver::run(&root, Verb::StopAll)?;
    if action == "wildcard" {
        realms::select(&root, Mode::Wildcard)?;
    }
    multiworld::set_enabled(&root, true)?;
    let result = (|| {
        let out = driver::run(&root, Verb::StartAll)?;
        if !out.ok {
            return Err(Error::Invalid(out.output));
        }
        let first = process::observe(&root, &coa_core::layout::read_ports(&root));
        let second_root = multiworld::root(&root);
        let second = process::observe(&second_root, &coa_core::layout::read_ports(&second_root));
        assert_eq!(first.world.state, ServiceState::Running);
        assert_eq!(second.world.state, ServiceState::Running);
        assert_ne!(first.world.pid, second.world.pid);
        assert_ne!(first.world.port, second.world.port);
        assert!(second.mysql.pid.is_none() && second.auth.pid.is_none());
        let db = coa_core::db::Db::from_repack(&root, coa_core::db::Account::Admin)?;
        let realms = db.query(
            "SELECT id,port,flag FROM acore_auth.realmlist WHERE id IN (1,2) ORDER BY id;",
        )?;
        println!("Two distinct world PIDs; shared auth/MySQL; realm rows:\n{realms}");
        for mode in [Mode::Coa, Mode::Wildcard] {
            let problems = coa_core::schema_check::check(&db.clone().for_realm(mode), &root)?;
            println!("{}: {} database problems", mode.name(), problems.len());
            for p in problems.iter().take(5) {
                println!("{}.{}.{}: {}", p.database, p.table, p.column, p.detail);
            }
        }
        assert!(multiworld::set_enabled(&root, false).is_err());
        assert!(realms::select(&root, Mode::Wildcard).is_err());
        Ok(())
    })();
    let stopped = driver::run(&root, Verb::StopAll)?;
    if !stopped.ok {
        return Err(Error::Invalid(stopped.output));
    }
    assert!(!multiworld::is_running(&root));
    assert_eq!(
        process::observe(&root, &coa_core::layout::read_ports(&root))
            .mysql
            .state,
        ServiceState::Stopped
    );
    result?;
    println!("Both worlds shut down before the shared database.");
    Ok(())
}
