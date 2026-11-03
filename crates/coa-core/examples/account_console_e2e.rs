use coa_core::{
    db::{Account, Db},
    driver::{self, Verb},
    ra::Ra,
    Result,
};
use std::path::PathBuf;
fn main() -> Result<()> {
    let root = PathBuf::from(std::env::args().nth(1).expect("disposable fixture"));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("coa-update-recovery-fixture-"));
    assert!(root.join("recovery-e2e-started").is_file());
    let result = std::panic::catch_unwind(|| -> Result<()> {
        let started = driver::run(&root, Verb::StartAll)?;
        assert!(started.ok, "{}", started.output);
        assert!(root.join("Core/Crashes").is_dir());
        let db = Db::from_repack(&root, Account::Admin)?;
        println!("Account acceptance: server started, connecting to RA");
        let mut ra = Ra::connect(&root)?;
        println!("Account acceptance: RA authenticated");
        let name = "STABILITYACCT2";
        assert_eq!(
            db.query("SELECT COUNT(*) FROM acore_auth.account WHERE username='STABILITYACCT2';")?
                .trim(),
            "0"
        );
        println!("Account acceptance: sending create command");
        coa_core::accounts::create(&root, name, "PrivateTest773", true)?;
        println!("Account acceptance: created");
        assert_eq!(db.query("SELECT COUNT(*) FROM acore_auth.account WHERE username='STABILITYACCT2' AND LENGTH(salt)=32 AND LENGTH(verifier)=32;")?.trim(), "1");
        assert!(ra
            .create_account(name, "PrivateTest773")
            .unwrap_err()
            .to_string()
            .contains("already taken"));
        ra.set_account_password(name, "PrivateTest774")?;
        ra.set_account_access(name, 3)?;
        assert_eq!(db.query("SELECT gmlevel FROM acore_auth.account_access WHERE id=(SELECT id FROM acore_auth.account WHERE username='STABILITYACCT2');")?.trim(), "3");
        ra.delete_account(name)?;
        assert_eq!(
            db.query("SELECT COUNT(*) FROM acore_auth.account WHERE username='STABILITYACCT2';")?
                .trim(),
            "0"
        );
        lost_reply(&root, &db)?;
        println!("PASS: native RA account create, SRP salt/verifier, duplicate refusal, password change, administrator access and deletion");
        Ok(())
    });
    let stopped = driver::run(&root, Verb::StopAll)?;
    assert!(stopped.ok, "{}", stopped.output);
    result.map_err(|_| {
        coa_core::Error::Invalid("Native account acceptance failed; fixture stopped".into())
    })?
}

fn lost_reply(root: &std::path::Path, db: &Db) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    let path = root.join("Settings/repack.json");
    let original = std::fs::read(&path)?;
    struct Restore(std::path::PathBuf, Vec<u8>);
    impl Drop for Restore {
        fn drop(&mut self) {
            std::fs::write(&self.0, &self.1).expect("restore disposable fixture settings");
        }
    }
    let restore = Restore(path.clone(), original.clone());
    let actual = coa_core::layout::read_ports(root).ra;
    for (name, forward, ready) in [
        ("STABLOSTREPLY", true, true),
        ("STABNOTCREATED", false, true),
        ("STABNOTREADY", false, false),
    ] {
        assert_eq!(
            db.query(&format!(
                "SELECT COUNT(*) FROM acore_auth.account WHERE username='{name}';"
            ))?
            .trim(),
            "0"
        );
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let mut config: serde_json::Value = serde_json::from_slice(&original)?;
        config["raPort"] = listener.local_addr()?.port().into();
        coa_core::fsx::atomic_write_json(&path, &config)?;
        let proxy = std::thread::spawn(move || {
            fn until(s: &mut TcpStream, marker: &[u8]) -> Vec<u8> {
                s.set_read_timeout(Some(std::time::Duration::from_secs(30)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(marker) {
                    s.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                bytes
            }
            let (mut client, _) = listener.accept().unwrap();
            let mut server = TcpStream::connect(("127.0.0.1", actual)).unwrap();
            for marker in [b"Username: ".as_slice(), b"Password: ".as_slice()] {
                client.write_all(&until(&mut server, marker)).unwrap();
                server.write_all(&until(&mut client, b"\r\n")).unwrap();
            }
            client.write_all(&until(&mut server, b"AC>")).unwrap();
            let readiness = until(&mut client, b"\r\n");
            assert_eq!(readiness, b"server info\r\n");
            if !ready {
                return;
            }
            server.write_all(&readiness).unwrap();
            client.write_all(&until(&mut server, b"AC>")).unwrap();
            let command = until(&mut client, b"\r\n");
            if forward {
                server.write_all(&command).unwrap();
                let _ = until(&mut server, b"AC>");
            }
        });
        let outcome = coa_core::accounts::create(root, name, "PrivateTest773", false);
        proxy.join().unwrap();
        std::fs::write(&path, &original)?;
        if forward {
            outcome?;
            assert_eq!(
                db.query(&format!(
                    "SELECT COUNT(*) FROM acore_auth.account WHERE username='{name}';"
                ))?
                .trim(),
                "1"
            );
            assert!(coa_core::accounts::create(root, name, "DifferentPass77", false).is_err());
            Ra::connect(root)?.delete_account(name)?;
        } else {
            assert!(outcome.is_err());
            if !ready {
                assert!(outcome
                    .unwrap_err()
                    .to_string()
                    .contains("No create command was sent"));
            }
            assert_eq!(
                db.query(&format!(
                    "SELECT COUNT(*) FROM acore_auth.account WHERE username='{name}';"
                ))?
                .trim(),
                "0"
            );
        }
    }
    drop(restore);
    println!("PASS: a lost native console reply is reconciled by exact SRP credentials; a command that was never executed is rejected");
    Ok(())
}
