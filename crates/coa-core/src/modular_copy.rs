//! A modular copy of a classic server, next to it.
//!
//! A server with the classic core (every module compiled into `worldserver.exe`) is never converted in place.
//! The Modules Preview installs a new server beside it and copies the old one's data into it: every game
//! database (accounts, characters, world, bots) and the module settings. The old server is only read:
//!
//! * if its database is running, the schemas are dumped from it (consistent snapshots, nothing is written);
//! * if it is stopped, its data files are copied to a private folder and read by a temporary database
//!   process of the NEW server; nothing of the old server is started or executed.
//!
//! The new server then takes the preview core like any other server (the caller applies the update).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::db::{Account, Db};
use crate::error::{Error, Result};
use crate::fsx;
use crate::process::ServiceState;

/// Whether `root` runs the classic core: a Windows repack whose `worldserver.exe` carries no plugin-host
/// fingerprint.
pub fn is_classic(root: &Path) -> bool {
    !crate::docker::is_docker(root)
        && root.join("Core/worldserver.exe").is_file()
        && crate::provenance::realm_host_fingerprint(root).is_none()
}

#[derive(Debug, Clone, Serialize)]
pub struct CopyReport {
    pub schemas: Vec<String>,
    pub accounts: u64,
    pub characters: u64,
    pub config_files: Vec<String>,
    /// How the old data was read: "running database" or "copy of the stopped database".
    pub read_from: String,
}

/// Game schemas of a CoA server: `acore_*` (auth, characters, world, their Wildcard/Reborn variants, bots).
fn game_schemas(db: &Db) -> Result<Vec<String>> {
    let mut out: Vec<String> = db
        .query("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME LIKE 'acore\\_%' ORDER BY SCHEMA_NAME;")?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        // work schemas a restore leaves behind are not game data
        .filter(|s| !s.contains("_restore_") && !s.contains("_before_restore_"))
        .collect();
    out.dedup();
    if !out.iter().any(|s| s == "acore_auth") || !out.iter().any(|s| s == "acore_characters") {
        return Err(Error::Invalid("The old server's database has no accounts or characters schema.".into()));
    }
    Ok(out)
}

fn count(db: &Db, sql: &str) -> u64 {
    db.query(sql).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// A database process of the new server reading a private copy of the old server's stopped data files.
struct TemporaryDatabase {
    child: Child,
    dir: PathBuf,
    db: Option<Db>,
    stopped: bool,
}

impl TemporaryDatabase {
    /// Shut it down cleanly, force it if it does not go, and remove the private copy. Fails if the process is
    /// still there afterwards, so a leftover database never holds the new server's files.
    fn stop(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        if let Some(db) = &self.db {
            let _ = db.shutdown_server();
        }
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            match self.child.try_wait()? {
                Some(_) => break,
                None if Instant::now() > deadline => {
                    tracing::warn!(pid = self.child.id(), "temporary database did not shut down; terminating it");
                    self.child.kill()?;
                    self.child.wait()?;
                    break;
                }
                None => std::thread::sleep(Duration::from_millis(250)),
            }
        }
        self.stopped = true;
        fs::remove_dir_all(&self.dir)?;
        Ok(())
    }
}

impl Drop for TemporaryDatabase {
    fn drop(&mut self) {
        if !self.stopped {
            if let Err(e) = self.stop() {
                tracing::warn!(error = %e, pid = self.child.id(), "temporary database left behind");
            }
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<u64> {
    let mut bytes = 0;
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let md = fs::symlink_metadata(entry.path())?;
        if md.file_type().is_symlink() {
            return Err(Error::Invalid(format!("{} is a link; the database folder is not copied.", entry.path().display())));
        }
        let target = to.join(entry.file_name());
        if md.is_dir() {
            bytes += copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
            bytes += md.len();
        }
    }
    Ok(bytes)
}

fn dir_size(dir: &Path) -> u64 {
    fs::read_dir(dir).map(|rd| rd.flatten().map(|e| {
        let p = e.path();
        if p.is_dir() { dir_size(&p) } else { e.metadata().map(|m| m.len()).unwrap_or(0) }
    }).sum()).unwrap_or(0)
}

fn start_temporary(old: &Path, new: &Path, work: &Path, progress: &dyn Fn(&str)) -> Result<(TemporaryDatabase, Db)> {
    let source = old.join("mysql/data");
    if !source.join("mysql").is_dir() || !source.join("ibdata1").is_file() {
        return Err(Error::Invalid("The old server's database folder is incomplete.".into()));
    }
    let dir = work.join("old-database");
    let _ = fs::remove_dir_all(&dir);
    fsx::require_space(work, dir_size(&source))?;
    progress("Copying the old server's database files");
    let data = dir.join("data");
    copy_tree(&source, &data)?;
    let (root_password, _) = crate::db::credentials(old)?;
    let bin = new.join("mysql/bin");
    let port = crate::install::free_port()?;
    let child = Command::new(bin.join("mysqld.exe"))
        .arg("--no-defaults")
        .arg(format!("--basedir={}", new.join("mysql").display()))
        .arg(format!("--datadir={}", data.display()))
        .arg(format!("--port={port}"))
        .args(["--bind-address=127.0.0.1", "--mysqlx=0", "--skip-log-bin", "--character-set-server=utf8mb4",
            "--collation-server=utf8mb4_unicode_ci", "--max_allowed_packet=128M"])
        .arg(format!("--log-error={}", dir.join("mysql-error.log").display()))
        .arg(format!("--pid-file={}", dir.join("mysql.pid").display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags_hidden()
        .spawn()?;
    let db = Db::with_tools(bin, port, "root", &root_password, crate::realms::Mode::Coa);
    let mut temp = TemporaryDatabase { child, dir, db: Some(db.clone()), stopped: false };
    progress("Opening the copy of the old database");
    let deadline = Instant::now() + Duration::from_secs(300);
    while !db.ping() {
        if let Ok(Some(status)) = temp.child.try_wait() {
            let log = fs::read_to_string(temp.dir.join("mysql-error.log")).unwrap_or_default();
            let tail: Vec<&str> = log.lines().rev().take(8).collect();
            return Err(Error::Invalid(format!(
                "The copy of the old database did not open ({status}). {}",
                tail.into_iter().rev().collect::<Vec<_>>().join(" ")
            )));
        }
        if Instant::now() > deadline {
            return Err(Error::Invalid("The copy of the old database did not open within 5 minutes.".into()));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    Ok((temp, db))
}

trait HiddenWindow {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

impl HiddenWindow for Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(0x0800_0000);
        }
        self
    }
}

/// Settings the owner may have changed that the new core can take as they are: the module configs
/// (`Core/configs/modules/*.conf`; the shipped `.dist` files belong to the new core) and the realm profiles.
/// `worldserver.conf` and `authserver.conf` are rendered by the launcher from the new server's own
/// templates at every start, so they are not copied.
fn settings_files(old: &Path) -> Vec<String> {
    crate::backup::config_files(old)
        .into_iter()
        .filter(|f| {
            (f.starts_with("Core/configs/modules/") && f.ends_with(".conf") && !f.ends_with("/mod_ascension_compat.conf"))
                || f.starts_with("Settings/realm-profile")
        })
        .collect()
}

/// Copy the old server's data into `new`, a server just installed and not yet started by anyone.
pub fn copy_data(old: &Path, new: &Path, progress: &dyn Fn(&str)) -> Result<CopyReport> {
    if !is_classic(old) {
        return Err(Error::Invalid("Only a server with the classic core is copied.".into()));
    }
    if fsx::canonicalize_lenient(old)? == fsx::canonicalize_lenient(new)? {
        return Err(Error::Invalid("The new server must be in another folder.".into()));
    }
    let work = crate::registry::metadata_dir_for(new)?.join("import");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;

    // 1. Read the old databases.
    let observed = crate::process::observe(old, &crate::layout::read_ports(old));
    let running = observed.mysql.state == ServiceState::Running
        && Db::from_repack(old, Account::Admin).map(|db| db.ping()).unwrap_or(false);
    let mut temporary: Option<TemporaryDatabase> = None;
    let (source, read_from) = if running {
        (Db::from_repack(old, Account::Admin)?.for_realm(crate::realms::Mode::Coa), "running database")
    } else {
        let (temp, db) = start_temporary(old, new, &work, progress)?;
        temporary = Some(temp);
        (db, "copy of the stopped database")
    };
    let schemas = game_schemas(&source)?;
    let accounts = count(&source, "SELECT COUNT(*) FROM acore_auth.account;");
    let characters = count(&source, "SELECT COUNT(*) FROM acore_characters.characters;");
    let mut dumps = Vec::new();
    for schema in &schemas {
        progress(&format!("Copying {schema}"));
        let file = work.join(format!("{schema}.sql.zst"));
        source.dump_to(schema, &file)?;
        dumps.push((schema.clone(), file));
    }
    // The private copy of the old data files is not needed any more; its database must be gone before the new
    // server's own database starts.
    if let Some(mut temp) = temporary.take() {
        progress("Closing the copy of the old database");
        temp.stop()?;
    }

    // 2. Put them into the new server, on ports nobody uses, so a running old server is not disturbed.
    crate::install::with_scratch_ports(new, || {
        crate::backup::with_database(new, |db| {
            let db = db.clone().for_realm(crate::realms::Mode::Coa);
            for (schema, file) in &dumps {
                progress(&format!("Importing {schema}"));
                db.query(&format!(
                    "DROP DATABASE IF EXISTS `{schema}`; CREATE DATABASE `{schema}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"
                ))?;
                db.import_from(schema, file)?;
            }
            let (a, c) = (
                count(&db, "SELECT COUNT(*) FROM acore_auth.account;"),
                count(&db, "SELECT COUNT(*) FROM acore_characters.characters;"),
            );
            if (a, c) != (accounts, characters) {
                return Err(Error::Invalid(format!(
                    "The copy is incomplete: {a} of {accounts} accounts and {c} of {characters} characters arrived."
                )));
            }
            Ok(())
        })
    })?;

    // 3. Settings: module configs and realm profiles; ports and console login as the old server had them
    //    (the console account lives in the copied accounts database).
    progress("Copying settings");
    let files = settings_files(old);
    for rel in &files {
        let from = fsx::ensure_within(old, &fsx::safe_join(old, rel)?)?;
        let to = fsx::ensure_within(new, &fsx::safe_join(new, rel)?)?;
        fsx::atomic_write(&to, &fs::read(from)?)?;
    }
    let old_repack: serde_json::Value = fsx::read_json(&old.join("Settings/repack.json"))?;
    let mut repack: serde_json::Value = fsx::read_json(&new.join("Settings/repack.json"))?;
    if let (Some(dst), Some(src)) = (repack.as_object_mut(), old_repack.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    fsx::atomic_write_json(&new.join("Settings/repack.json"), &repack)?;
    let _ = fs::remove_dir_all(&work);
    Ok(CopyReport { schemas, accounts, characters, config_files: files, read_from: read_from.into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_owner_settings_are_copied() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path();
        for rel in [
            "Core/configs/worldserver.conf",
            "Core/configs/worldserver.conf.dist",
            "Core/configs/modules/mod_x.conf",
            "Core/configs/modules/mod_x.conf.dist",
            "Core/configs/modules/mod_ascension_compat.conf",
            "Settings/repack.json",
            "Settings/worldserver.conf.template",
            "Settings/realm-profile.json",
        ] {
            let p = old.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }
        let mut got = settings_files(old);
        got.sort();
        assert_eq!(got, vec![
            "Core/configs/modules/mod_x.conf".to_string(),
            "Settings/realm-profile.json".to_string(),
        ]);
    }

    #[test]
    fn a_folder_without_a_core_is_not_classic() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_classic(dir.path()));
    }
}
