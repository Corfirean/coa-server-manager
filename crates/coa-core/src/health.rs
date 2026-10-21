//! Turn worldserver/authserver log output into a stable cause, so users never see raw exit codes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::ErrorCode;

/// Best-effort cause of a failed startup from log text. Later lines win over earlier ones.
pub fn diagnose(text: &str) -> Option<ErrorCode> {
    let mut found = None;
    for line in text.lines() {
        let l = line.to_lowercase();
        let hit = if l.contains("database structure is not up to date")
            || (l.contains("table '") && l.contains("doesn't exist") && l.contains("[1146]"))
            || l.contains("could not prepare statements")
        {
            Some(ErrorCode::WorldDbIncompatible)
        } else if l.contains("can't connect to mysql")
            || l.contains("can't connect to server on")
            || l.contains("[2003]")
            || l.contains("[2002]")
            || l.contains("connection refused")
        {
            Some(ErrorCode::DatabaseNotRunning)
        } else if l.contains("address already in use") || l.contains("could not bind") || l.contains("only one usage of each socket") {
            Some(ErrorCode::PortInUse)
        } else if l.contains("does not hold the coa client dbc set") || l.contains("_outdated_ dbc data") {
            Some(ErrorCode::GameDataMismatch)
        } else if l.contains("config::loadfile") && l.contains("failed open file") && l.contains("worldserver.conf") {
            Some(ErrorCode::ServerFilesIncomplete)
        } else {
            None
        };
        if hit.is_some() {
            found = hit;
        }
    }
    found
}

pub fn tail(path: &Path, max_bytes: u64) -> String {
    let Ok(mut f) = File::open(path) else { return String::new() };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return String::new() };
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max_bytes)));
    let mut buf = Vec::new();
    let _ = f.take(max_bytes).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Diagnose from the repack's own service logs (only the recent tail of each).
pub fn diagnose_installation(root: &Path) -> Option<ErrorCode> {
    ["Core/Logs/world-console.log", "Core/Logs/auth-console.log", "mysql/logs/mysql-error.log"]
        .iter()
        .find_map(|rel| diagnose(&tail(&root.join(rel), 64 * 1024)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_data_folder_without_the_coa_dbc_set_is_named_as_such() {
        let log = "Using DataDir /srv/data/\nSpellAffect.dbc not found in /srv/data/dbc/\nDataDir does not hold the CoA client DBC set. Install it with apps/coa-dbc/client_dbc.py.";
        assert_eq!(diagnose(log), Some(ErrorCode::GameDataMismatch));
        assert_eq!(diagnose("You have _outdated_ DBC data. Please extract correct versions from current using client."), Some(ErrorCode::GameDataMismatch));
    }

    #[test]
    fn recognises_missing_migrations() {
        let log = "ERROR [sql.sql] [1146] Table 'acore_world.coa_boss' doesn't exist\nFATAL [sql.sql] Your database structure is not up to date.";
        assert_eq!(diagnose(log), Some(ErrorCode::WorldDbIncompatible));
    }

    #[test]
    fn recognises_db_down_and_port_conflicts() {
        assert_eq!(diagnose("ERROR Can't connect to MySQL server on '127.0.0.1:3307'"), Some(ErrorCode::DatabaseNotRunning));
        assert_eq!(diagnose("Could not bind to port 8085"), Some(ErrorCode::PortInUse));
        assert_eq!(diagnose("INFO all good"), None);
    }

    #[test]
    fn tail_reads_only_the_end_of_huge_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.log");
        let mut s = "x".repeat(200_000);
        s.push_str("\nFATAL Your database structure is not up to date\n");
        std::fs::write(&p, s).unwrap();
        let t = tail(&p, 1000);
        assert!(t.len() <= 1000);
        assert_eq!(diagnose(&t), Some(ErrorCode::WorldDbIncompatible));
        assert_eq!(tail(&dir.path().join("missing"), 10), "");
    }
}
