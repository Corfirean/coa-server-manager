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
    /// World settings the owner changed (Server Configuration) that were set on the new server too.
    pub world_settings: Vec<String>,
    /// Changed world settings the new server's configuration does not take, with the reason.
    pub world_settings_skipped: Vec<String>,
}

/// What a modular copy of `old` would take, read without changing anything: the old server is never started,
/// and its database is only queried when it is already running.
#[derive(Debug, Clone, Serialize)]
pub struct CopyPreview {
    pub path: PathBuf,
    /// The classic core (modules compiled in): the only kind that is copied.
    pub classic: bool,
    /// Already a modular core: it is added as it is, not copied.
    pub modular: bool,
    pub version: Option<String>,
    /// "running", "stopped" or "missing" (no database folder).
    pub database: String,
    /// Read from the running database; `None` when it is stopped (they are counted while copying).
    pub accounts: Option<u64>,
    pub characters: Option<u64>,
    pub schemas: Vec<String>,
    /// Size of the old database files; a stopped database is copied once more while it is read.
    pub database_bytes: u64,
    /// Settings files that go along (module configs, realm profiles).
    pub config_files: Vec<String>,
    /// Extra worlds of the old server (`.realms/<name>`). Their databases are in the same MySQL and go along, and
    /// the copied realm profile switches them on; the new server prepares their folders when it starts them.
    pub extra_worlds: Vec<String>,
}

pub fn preview(old: &Path) -> Result<CopyPreview> {
    if !old.is_dir() {
        return Err(Error::Invalid(format!("{} is not a folder.", old.display())));
    }
    let classic = is_classic(old);
    let modular = !crate::docker::is_docker(old)
        && old.join("Core/worldserver.exe").is_file()
        && crate::provenance::realm_host_fingerprint(old).is_some();
    let version = crate::registry::metadata_dir_for(old)
        .ok()
        .and_then(|dir| crate::registry::MetaDir::open(&dir).ok())
        .and_then(|(_, meta)| meta.core.version);
    let data = old.join("mysql/data");
    let has_data = data.join("mysql").is_dir();
    let observed = crate::process::observe(old, &crate::layout::read_ports(old));
    let running_db = (has_data && observed.mysql.state == ServiceState::Running)
        .then(|| Db::from_repack(old, Account::Admin).ok())
        .flatten()
        .filter(|db| db.ping())
        .map(|db| db.for_realm(crate::realms::Mode::Coa));
    let (database, accounts, characters, schemas) = match &running_db {
        Some(db) => (
            "running",
            Some(count(db, "SELECT COUNT(*) FROM acore_auth.account;")),
            Some(count(db, "SELECT COUNT(*) FROM acore_characters.characters;")),
            game_schemas(db).unwrap_or_default(),
        ),
        None if has_data => ("stopped", None, None, Vec::new()),
        None => ("missing", None, None, Vec::new()),
    };
    let mut extra_worlds: Vec<String> = fs::read_dir(old.join(".realms"))
        .map(|rd| rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    extra_worlds.sort();
    Ok(CopyPreview {
        path: old.to_path_buf(),
        classic,
        modular,
        version,
        database: database.into(),
        accounts,
        characters,
        schemas,
        database_bytes: if has_data { dir_size(&data) } else { 0 },
        config_files: settings_files(old),
        extra_worlds,
    })
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
    progress("Copying world settings");
    let (world_settings, world_settings_skipped) = carry_world_settings(old, new)?;
    Ok(CopyReport {
        schemas,
        accounts,
        characters,
        config_files: files,
        read_from: read_from.into(),
        world_settings,
        world_settings_skipped,
    })
}

/// The world settings changed on the old server (they live in its `Settings/worldserver.conf.template`, which the new
/// server does not take: it keeps the template of its own version) are set on the new server through the same checks
/// the Server Configuration page uses. Settings the Manager owns (ports, folders, passwords) are not among them.
///
/// The shipped template already differs from `worldserver.conf.dist` in many places, so "changed" means "differs from
/// what the new server has now": for a copy onto the same version that is exactly what the owner changed. (A value the
/// owner never touched but a newer release ships differently cannot be told apart from an owner's change; it keeps the
/// old server's value, which is what that server ran with.)
fn carry_world_settings(old: &Path, new: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let current: std::collections::BTreeMap<String, String> =
        crate::allsettings::list(new)?.into_iter().map(|i| (i.key, i.value)).collect();
    let mut changes = std::collections::BTreeMap::new();
    let mut skipped = Vec::new();
    for item in crate::allsettings::list(old)? {
        match current.get(&item.key) {
            Some(now) if *now == item.value => {}
            Some(_) => {
                changes.insert(item.key, item.value);
            }
            None if item.changed => skipped.push(format!("{}: not a setting of the new version", item.key)),
            None => {}
        }
    }
    let meta = crate::registry::metadata_dir_for(new)?;
    let mut carried = Vec::new();
    match crate::allsettings::save(new, &meta, &changes) {
        Ok(set) => carried.extend(set),
        // One value the new version refuses must not lose the others: set them one at a time.
        Err(_) => {
            for (key, value) in changes {
                match crate::allsettings::save(new, &meta, &std::collections::BTreeMap::from([(key.clone(), value)])) {
                    Ok(set) => carried.extend(set),
                    Err(e) => skipped.push(format!("{key}: {e}")),
                }
            }
        }
    }
    Ok((carried, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIST: &str = "[worldserver]\r\n\r\n#\r\n#    Rate.Drop.Item.Epic\r\n#        Description: Epic drops.\r\n#        Default:     1\r\n#\r\n\r\nRate.Drop.Item.Epic = 1\r\n\r\n#\r\n#    LoginDatabaseInfo\r\n#        Description: secret\r\n#\r\n\r\nLoginDatabaseInfo = \"127.0.0.1;3306;a;b;c\"\r\n\r\n#\r\n#    Instance.ResetTimeHour\r\n#        Description: Hour of the reset.\r\n#        Default:     4\r\n#\r\n\r\nInstance.ResetTimeHour = 4\r\n";

    fn world(root: &Path, dist: &str, conf: &str) {
        fs::create_dir_all(root.join("Core/configs")).unwrap();
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::create_dir_all(crate::registry::metadata_dir_for(root).unwrap()).unwrap();
        fs::write(root.join("Core/configs/worldserver.conf.dist"), dist).unwrap();
        fs::write(root.join("Settings/worldserver.conf.template"), conf).unwrap();
        fs::write(root.join("Core/configs/worldserver.conf"), conf).unwrap();
    }

    #[test]
    fn changed_world_settings_go_to_the_new_server_and_the_rest_keep_its_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("old"), dir.path().join("new"));
        let old_dist = format!("{DIST}\r\n#\r\n#    Old.Only\r\n#        Description: gone in the new version.\r\n#        Default:     1\r\n#\r\n\r\nOld.Only = 1\r\n");
        world(&old, &old_dist, "[worldserver]\r\nRate.Drop.Item.Epic = 3\r\nOld.Only = 2\r\nLoginDatabaseInfo = \"old\"\r\n");
        world(&new, DIST, "[worldserver]\r\nLoginDatabaseInfo = \"new\"\r\n");

        let (carried, skipped) = carry_world_settings(&old, &new).unwrap();
        assert_eq!(carried, ["Rate.Drop.Item.Epic"]);
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert!(skipped[0].starts_with("Old.Only"));
        for f in ["Settings/worldserver.conf.template", "Core/configs/worldserver.conf"] {
            let text = fs::read_to_string(new.join(f)).unwrap();
            assert!(text.contains("Rate.Drop.Item.Epic = 3"), "{f}: {text}");
            assert!(text.contains("LoginDatabaseInfo = \"new\""), "the Manager's own settings stay: {text}");
            assert!(!text.contains("Old.Only"), "{text}");
        }
    }

    #[test]
    fn the_preview_of_a_folder_reads_only_files() {
        let dir = tempfile::tempdir().unwrap();
        let classic = dir.path().join("classic");
        fs::create_dir_all(classic.join("Core/configs/modules")).unwrap();
        fs::create_dir_all(classic.join("mysql/data/mysql")).unwrap();
        fs::create_dir_all(classic.join(".realms/secondary")).unwrap();
        fs::write(classic.join("Core/worldserver.exe"), b"not a plugin host").unwrap();
        fs::write(classic.join("Core/configs/modules/mod_x.conf"), b"x").unwrap();
        fs::write(classic.join("mysql/data/ibdata1"), vec![0u8; 2048]).unwrap();
        let p = preview(&classic).unwrap();
        assert!(p.classic && !p.modular);
        assert_eq!(p.database, "stopped");
        assert_eq!((p.accounts, p.characters), (None, None), "a stopped database is not opened");
        assert_eq!(p.database_bytes, 2048);
        assert_eq!(p.config_files, ["Core/configs/modules/mod_x.conf"]);
        assert_eq!(p.extra_worlds, ["secondary"]);

        let empty = dir.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        let p = preview(&empty).unwrap();
        assert!(!p.classic && !p.modular);
        assert_eq!(p.database, "missing");
        assert!(preview(&dir.path().join("nowhere")).is_err());
    }

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
