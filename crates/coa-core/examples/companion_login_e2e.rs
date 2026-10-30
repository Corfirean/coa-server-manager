use coa_core::{
    db::{Account, Db},
    driver::{self, Verb},
    ra::Ra,
    Result,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("disposable recovery fixture"),
    );
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("coa-update-recovery-fixture-"));
    assert!(root.join("recovery-e2e-started").is_file());
    let result = std::panic::catch_unwind(|| -> Result<()> {
        let start = driver::run(&root, Verb::StartAll)?;
        assert!(start.ok, "{}", start.output);
        let db = Db::from_repack(&root, Account::Admin)?;
        coa_core::companions::ensure_templates(&db)?;
        let mut ra = Ra::connect(&root)?;
        let before = coa_core::population::query(&root)?;
        assert_eq!(before.bots_online, 0);
        let rejected = db.query("SELECT name, at_login FROM acore_characters.characters WHERE guid=3030 AND account IN (SELECT id FROM acore_auth.account WHERE username LIKE 'COABOTHOST%');")?;
        assert!(rejected.contains("Christian Shadowheart"));
        assert_eq!(rejected.split_whitespace().last(), Some("1"));
        println!("Repair reply: {}", ra.run("botcmd spawnbot 3030")?);
        let repair_deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let row = db.query(
                "SELECT name, at_login, online FROM acore_characters.characters WHERE guid=3030;",
            )?;
            if row.trim().ends_with("0\t1") {
                assert!(!row.contains("Christian Shadowheart"));
                println!("PASS: the exact previously rejected companion was renamed by server validation and logged in");
                break;
            }
            assert!(
                Instant::now() < repair_deadline,
                "previously rejected companion logged in"
            );
            std::thread::sleep(Duration::from_secs(1));
        }
        let _ = ra.despawn_all()?;
        let clear_deadline = Instant::now() + Duration::from_secs(30);
        while coa_core::population::query(&root)?.bots_online > 0 {
            assert!(Instant::now() < clear_deadline);
            std::thread::sleep(Duration::from_secs(1));
        }
        println!("Spawn reply: {}", ra.spawn_bots(21)?);
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let pop = coa_core::population::query(&root)?;
            if pop.bots_online >= 21 {
                println!(
                    "PASS: {} companions logged in; real players={}",
                    pop.bots_online, pop.players_online
                );
                break;
            }
            if Instant::now() > deadline {
                return Err(coa_core::Error::Invalid(format!(
                    "Only {} of 21 companion logins completed",
                    pop.bots_online
                )));
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        let _ = ra.cancel_spawning()?;
        let _ = ra.despawn_all()?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while coa_core::population::query(&root)?.bots_online > 0 {
            assert!(Instant::now() < deadline, "companions despawned");
            std::thread::sleep(Duration::from_secs(1));
        }
        println!("PASS: companion despawn completed");
        Ok(())
    });
    let stopped = driver::run(&root, Verb::StopAll)?;
    assert!(stopped.ok, "{}", stopped.output);
    result.map_err(|_| {
        coa_core::Error::Invalid("Companion native acceptance panicked; fixture stopped".into())
    })?
}
