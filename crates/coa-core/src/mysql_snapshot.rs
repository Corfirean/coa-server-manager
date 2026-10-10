use std::{collections::{BTreeMap, BTreeSet}, fs, path::{Path, PathBuf}};
use std::io::{BufReader, BufWriter, Write};
use serde::{Deserialize, Serialize};
use crate::{fsx, Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Snapshot {
    pub path: String,
    pub files: BTreeMap<String, String>,
    pub directories: BTreeSet<String>,
    pub server_sha256: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_sha256: Option<String>,
    #[serde(default)]
    pub expanded_bytes: u64,
}

fn reject_link(metadata: &fs::Metadata) -> Result<()> {
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)] let linked = { use std::os::windows::fs::MetadataExt; linked || metadata.file_attributes() & 0x400 != 0 };
    if linked { return Err(Error::Invalid("Recovery copies cannot follow symbolic links or junctions.".into())); }
    Ok(())
}

fn directories(root: &Path) -> Result<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            reject_link(&fs::symlink_metadata(entry.path())?)?;
            let path = fsx::ensure_within(root, &entry.path())?;
            if entry.file_type()?.is_dir() {
                let relative = path.strip_prefix(root).map_err(|e| Error::Invalid(e.to_string()))?.to_string_lossy().replace('\\', "/");
                result.insert(relative);
                pending.push(path);
            }
        }
    }
    Ok(result)
}

fn copy_snapshot(point: &Path, snapshot: &Snapshot, target: &Path) -> Result<()> {
    if snapshot.archive_sha256.is_some() {
        verify(point, snapshot)?;
        if target.exists() { return Err(Error::Invalid("The recovery destination already exists.".into())); }
        let parent = target.parent().ok_or_else(|| Error::Invalid("Invalid recovery destination.".into()))?;
        fs::create_dir_all(parent)?;
        fsx::require_space(parent, snapshot.expanded_bytes)?;
        fs::create_dir(target)?;
        let input = fsx::ensure_within(point, &fsx::safe_join(point, &snapshot.path)?)?;
        let decoder = zstd::Decoder::new(BufReader::new(fs::File::open(input)?))?;
        let mut archive = tar::Archive::new(decoder);
        let mut seen = BTreeSet::new();
        let mut expanded = 0u64;
        for entry in archive.entries()? {
            let mut entry = entry?;
            let name = entry.path()?.to_string_lossy().trim_end_matches('/').to_string();
            if !seen.insert(name.clone()) { return Err(Error::Invalid("Duplicate entry in MySQL recovery archive.".into())); }
            let output = fsx::ensure_within(target, &fsx::safe_join(target, &name)?)?;
            if entry.header().entry_type().is_dir() && snapshot.directories.contains(&name) {
                fs::create_dir_all(output)?;
            } else if entry.header().entry_type().is_file() && snapshot.files.contains_key(&name) {
                expanded = expanded.checked_add(entry.size()).filter(|bytes| *bytes <= snapshot.expanded_bytes)
                    .ok_or_else(|| Error::Invalid("MySQL recovery archive exceeds its declared size.".into()))?;
                fs::create_dir_all(output.parent().unwrap())?;
                let mut file = fs::OpenOptions::new().write(true).create_new(true).open(output)?;
                if std::io::copy(&mut entry, &mut file)? != entry.size() { return Err(Error::Invalid("Truncated MySQL recovery file.".into())); }
                file.flush()?;
            } else { return Err(Error::Invalid("Unexpected entry in MySQL recovery archive.".into())); }
        }
        if expanded != snapshot.expanded_bytes || inventory(target)? != snapshot.files {
            return Err(Error::Invalid("MySQL recovery files failed verification.".into()));
        }
    } else { copy_verified(&point.join(&snapshot.path), target, &snapshot.files)?; }
    for directory in &snapshot.directories {
        fs::create_dir_all(fsx::ensure_within(target, &fsx::safe_join(target, directory)?)?)?;
    }
    if directories(target)? != snapshot.directories { return Err(Error::Invalid("The recovery directory inventory differs.".into())); }
    Ok(())
}

fn compress(source: &Path, point: &Path, snapshot: &mut Snapshot) -> Result<()> {
    snapshot.path = "mysql-data.tar.zst".into();
    let path = fsx::safe_join(point, &snapshot.path)?;
    fs::create_dir_all(point)?;
    fsx::require_space(point, snapshot.expanded_bytes)?;
    let file = fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    let encoder = zstd::Encoder::new(BufWriter::new(file), 3)?;
    let mut archive = tar::Builder::new(encoder);
    for directory in &snapshot.directories {
        archive.append_dir(directory, fsx::ensure_within(source, &fsx::safe_join(source, directory)?)?)?;
    }
    for name in snapshot.files.keys() {
        let input = fsx::ensure_within(source, &fsx::safe_join(source, name)?)?;
        archive.append_file(name, &mut fs::File::open(input)?)?;
    }
    archive.into_inner()?.finish()?.flush()?;
    snapshot.archive_sha256 = Some(fsx::sha256_file(&path)?);
    snapshot.bytes = fs::metadata(path)?.len();
    Ok(())
}

pub(crate) fn inventory(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            let path = fsx::ensure_within(root, &entry.path())?;
            reject_link(&metadata)?;
            if metadata.is_dir() { directories.push(path); continue; }
            if !metadata.is_file() { return Err(Error::Invalid("Unsupported database snapshot file.".into())); }
            let relative = path.strip_prefix(root).map_err(|e| Error::Invalid(e.to_string()))?.to_string_lossy().replace('\\', "/");
            fsx::safe_join(root, &relative)?;
            if relative.ends_with(".isl") { return Err(Error::Invalid("External MySQL tablespaces require a separate recovery method.".into())); }
            files.insert(relative, fsx::sha256_file(&path)?);
        }
    }
    Ok(files)
}

pub(crate) fn copy_verified(source: &Path, target: &Path, files: &BTreeMap<String, String>) -> Result<()> {
    if target.exists() { return Err(Error::Invalid("Database snapshot destination already exists.".into())); }
    let bytes = files.keys().try_fold(0u64, |total, name| -> Result<u64> {
        let path = fsx::ensure_within(source, &fsx::safe_join(source, name)?)?;
        total.checked_add(fs::metadata(path)?.len()).ok_or_else(|| Error::Invalid("Snapshot size overflow.".into()))
    })?;
    let parent = target.parent().ok_or_else(|| Error::Invalid("Invalid snapshot destination.".into()))?;
    fs::create_dir_all(parent)?;
    fsx::require_space(parent, bytes)?;
    fs::create_dir(target)?;
    for (relative, expected) in files {
        let source_file = fsx::ensure_within(source, &fsx::safe_join(source, relative)?)?;
        let target_file = fsx::ensure_within(target, &fsx::safe_join(target, relative)?)?;
        fs::create_dir_all(target_file.parent().unwrap())?;
        fs::copy(&source_file, &target_file)?;
        if fsx::sha256_file(&target_file)? != *expected { return Err(Error::Invalid(format!("Database snapshot file {relative} failed verification."))); }
    }
    if inventory(target)? != *files { return Err(Error::Invalid("Database snapshot inventory differs.".into())); }
    Ok(())
}

pub(crate) fn stop(root: &Path) -> Result<()> {
    let observed = crate::process::observe(root, &crate::layout::read_ports(root));
    if [observed.mysql.state, observed.auth.state, observed.world.state].iter().all(|state| *state == crate::process::ServiceState::Stopped)
        && !crate::multiworld::is_running(root) { return Ok(()); }
    let outcome = crate::driver::run(root, crate::driver::Verb::StopAll)?;
    let status = crate::process::observe(root, &crate::layout::read_ports(root));
    if !outcome.ok || [status.mysql.state, status.auth.state, status.world.state].iter().any(|state| *state != crate::process::ServiceState::Stopped)
        || crate::multiworld::is_running(root) {
        return Err(Error::Invalid("The complete server must be stopped before copying or restoring MySQL.".into()));
    }
    Ok(())
}

pub(crate) fn stop_previous_rehearsals(meta: &Path) -> Result<()> {
    let location = meta.join("rehearsals");
    if !location.exists() { return Ok(()); }
    for entry in fs::read_dir(&location)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() != 32 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) { continue; }
        let fixture = fsx::ensure_within(&location, &entry.path().join("server"))?;
        let metadata = crate::registry::metadata_dir_for(&fixture)?;
        if !metadata.join("install.json").is_file() { continue; }
        let (_, installed) = crate::registry::MetaDir::open(&metadata)?;
        if fsx::canonicalize_lenient(Path::new(&installed.server_path))? != fsx::canonicalize_lenient(&fixture)? {
            return Err(Error::Invalid("The interrupted rehearsal metadata does not match its private server.".into()));
        }
        stop(&fixture)?;
    }
    Ok(())
}

pub(crate) fn capture(root: &Path, partial: &Path) -> Result<Snapshot> {
    if crate::docker::is_docker(root) || !cfg!(windows) { return Err(Error::Invalid("Cold MySQL snapshots require a Windows repack.".into())); }
    crate::backup::with_database(root, |db| {
        if db.query("SELECT COUNT(*) FROM performance_schema.replication_connection_configuration;")?.trim() != "0" {
            return Err(Error::Invalid("A replicated MySQL instance requires a separate isolated recovery method.".into()));
        }
        let data = fsx::ensure_within(root, &root.join("mysql/data"))?;
        for path in db.query("SELECT PATH FROM information_schema.INNODB_DATAFILES;")?.lines() {
            fsx::ensure_within(&data, &data.join(path))?;
        }
        Ok(())
    })?;
    stop(root)?;
    let data = fsx::ensure_within(root, &root.join("mysql/data"))?;
    if !data.join("mysql").is_dir() || !data.join("ibdata1").is_file() {
        return Err(Error::Invalid("The bundled MySQL data directory is incomplete.".into()));
    }
    let files = inventory(&data)?;
    let bytes = files.keys().try_fold(0u64, |total, file| -> Result<u64> {
        total.checked_add(fs::metadata(fsx::safe_join(&data, file)?)?.len()).ok_or_else(|| Error::Invalid("Snapshot size overflow.".into()))
    })?;
    let mut snapshot = Snapshot { path: "mysql-data".into(), files, directories:directories(&data)?, server_sha256: fsx::sha256_file(&root.join("mysql/bin/mysqld.exe"))?, bytes, expanded_bytes:bytes, archive_sha256:None };
    compress(&data, partial, &mut snapshot)?;
    verify(partial, &snapshot)?;
    Ok(snapshot)
}

pub(crate) fn verify(point: &Path, snapshot: &Snapshot) -> Result<()> {
    if snapshot.files.is_empty() || snapshot.server_sha256.len() != 64 {
        return Err(Error::Invalid("Invalid cold MySQL recovery inventory.".into()));
    }
    let data = fsx::ensure_within(point, &fsx::safe_join(point, &snapshot.path)?)?;
    if let Some(hash) = &snapshot.archive_sha256 {
        if snapshot.path != "mysql-data.tar.zst" || snapshot.expanded_bytes == 0 || fsx::sha256_file(&data)? != *hash {
            return Err(Error::Invalid("The compressed MySQL recovery copy is damaged.".into()));
        }
        for name in snapshot.files.keys().chain(snapshot.directories.iter()) { fsx::safe_join(point, name)?; }
        return Ok(());
    }
    if snapshot.path != "mysql-data" { return Err(Error::Invalid("Unsupported MySQL recovery format.".into())); }
    if inventory(&data)? != snapshot.files { return Err(Error::Invalid("The cold MySQL recovery copy is damaged or incomplete.".into())); }
    if directories(&data)? != snapshot.directories { return Err(Error::Invalid("The cold MySQL recovery directories are incomplete.".into())); }
    Ok(())
}

pub(crate) fn rehearsal_copy(root: &Path, meta: &Path, point: &Path, snapshot: &Snapshot) -> Result<PathBuf> {
    verify(point, snapshot)?;
    let destination = meta.join("rehearsals").join(uuid::Uuid::new_v4().simple().to_string()).join("server");
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let raw = entry.path();
            let relative = raw.strip_prefix(root).map_err(|e| Error::Invalid(e.to_string()))?.to_string_lossy().replace('\\', "/");
            let lower = relative.to_ascii_lowercase();
            let first = lower.split('/').next().unwrap();
            if entry.file_type()?.is_dir() && !relative.contains('/')
                && !["core", "data", "scripts", "settings", "runtime", "mysql", ".realms", "bugreport", "extras", "licenses"].contains(&first) { continue; }
            if lower == "mysql/data" || lower.starts_with("mysql/data-")
                || lower.split('/').any(|part| part == ".state")
                || lower == "bugreport/reports" || lower == "core/logs" || lower == "mysql/logs" { continue; }
            let metadata = fs::symlink_metadata(&raw)?;
            reject_link(&metadata)?;
            let path = fsx::ensure_within(root, &raw)?;
            if metadata.is_dir() { directories.push(path); }
            else if metadata.is_file() { files.insert(relative, fsx::sha256_file(&path)?); }
            else { return Err(Error::Invalid("Unsupported rehearsal file.".into())); }
        }
    }
    copy_verified(root, &destination, &files)?;
    copy_snapshot(point, snapshot, &destination.join("mysql/data"))?;
    let (_, mut installed) = crate::registry::MetaDir::open(meta)?;
    installed.id = uuid::Uuid::new_v4().to_string();
    installed.server_path = destination.to_string_lossy().into_owned();
    installed.client_path = None;
    crate::registry::MetaDir::create(&destination, &installed)?;
    Ok(destination)
}

pub(crate) fn isolate_connections(root: &Path) -> Result<()> {
    for relative in ["Settings/worldserver.conf.template", "Settings/authserver.conf.template", "Core/configs/worldserver.conf", "Core/configs/authserver.conf"] {
        let path = fsx::ensure_within(root, &root.join(relative))?;
        if !path.is_file() {
            if relative.ends_with(".template") { return Err(Error::Invalid("The private update copy lacks connection templates.".into())); }
            continue;
        }
        let mut config = crate::config::parser::ConfFile::parse_bytes(&fs::read(&path)?)?;
        for key in ["LoginDatabaseInfo", "WorldDatabaseInfo", "CharacterDatabaseInfo"] {
            if relative.contains("worldserver") || key == "LoginDatabaseInfo" {
                config.set(key, &format!("\"@{key}@\""), &[]);
            }
        }
        config.set("BindIP", "\"127.0.0.1\"", &[]);
        if relative.contains("worldserver") {
            config.set("Ra.IP", "\"127.0.0.1\"", &[]);
            config.set("DataDir", "\"@DATA@\"", &[]);
        }
        fsx::atomic_write(&path, config.to_text().as_bytes())?;
    }
    Ok(())
}

pub(crate) fn unchanged(root: &Path, snapshot: &Snapshot) -> Result<()> {
    let data = fsx::ensure_within(root, &root.join("mysql/data"))?;
    if inventory(&data)? != snapshot.files || directories(&data)? != snapshot.directories {
        return Err(Error::Invalid("The installed database changed during the private rehearsal. Retry after stopping other database access.".into()));
    }
    Ok(())
}

pub(crate) fn validate_connections(templates: &Path, installation: &Path) -> Result<()> {
    let port = crate::layout::read_ports(installation).mysql.to_string();
    let (_, password) = crate::db::credentials(installation)?;
    let realm = crate::realms::state(installation)?.active;
    for relative in ["Settings/worldserver.conf.template", "Settings/authserver.conf.template"] {
        let path = fsx::ensure_within(templates, &templates.join(relative))?;
        let config = crate::config::parser::ConfFile::parse_bytes(&fs::read(path)?)?;
        for (key, kind) in [("LoginDatabaseInfo", "auth"), ("WorldDatabaseInfo", "world"), ("CharacterDatabaseInfo", "characters")] {
            if relative.contains("authserver") && kind != "auth" { continue; }
            let value = config.get(key).ok_or_else(|| Error::Invalid(format!("The update cannot isolate the missing {key} connection.")))?.trim().trim_matches('"');
            if value == format!("@{key}@") { continue; }
            let fields: Vec<_> = value.split(';').collect();
            if fields.len() != 5 || !matches!(fields[0], "127.0.0.1" | "localhost") || fields[1] != port || fields[2] != "acore" || fields[3] != password || fields[4] != realm.schema(kind)? {
                return Err(Error::Invalid(format!("The {key} connection does not use this repack's bundled database. A private update cannot safely qualify an external database.")));
            }
        }
    }
    Ok(())
}

// Keep the replaced data directory. If interrupted between renames, a retry can install a new verified copy.
pub(crate) fn swap(root: &Path, meta: &Path, point: &Path, snapshot: &Snapshot, fail_after_move: bool) -> Result<PathBuf> {
    verify(point, snapshot)?;
    if fsx::sha256_file(&root.join("mysql/bin/mysqld.exe"))? != snapshot.server_sha256 {
        return Err(Error::Invalid("The MySQL executable differs from the recovery point.".into()));
    }
    let mysql = fsx::ensure_within(root, &root.join("mysql"))?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let staging = fsx::ensure_within(&mysql, &mysql.join(format!("data-restore-{token}")))?;
    let previous = fsx::ensure_within(&mysql, &mysql.join(format!("data-before-restore-{token}")))?;
    let live = fsx::ensure_within(&mysql, &mysql.join("data"))?;
    copy_snapshot(point, snapshot, &staging)?;
    let journal = meta.join("diagnostics").join(format!("database-restore-{token}.json"));
    fsx::atomic_write_json(&journal, &serde_json::json!({"schema":1,"state":"prepared","previous":previous,"staging":staging}))?;
    if live.exists() { fs::rename(&live, &previous)?; }
    if fail_after_move { return Err(Error::Invalid("Simulated interruption after saving the previous MySQL directory.".into())); }
    fs::rename(&staging, &live)?;
    fsx::atomic_write_json(&journal, &serde_json::json!({"schema":1,"state":"restored","previous":previous}))?;
    Ok(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    #[ignore = "requires COA_MYSQL_TEST_BIN pointing to the bundled MySQL tools"]
    fn real_mysql_restores_objects_users_rows_and_missing_defaults() {
        use std::process::{Child, Command, Stdio};
        use std::os::windows::process::CommandExt;
        struct Server(Child);
        impl Drop for Server { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
        let bin = PathBuf::from(std::env::var("COA_MYSQL_TEST_BIN").unwrap());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("server");
        let meta = temp.path().join("metadata");
        let point = temp.path().join("point");
        copy_verified(&bin, &root.join("mysql/bin"), &inventory(&bin).unwrap()).unwrap();
        let data = root.join("mysql/data");
        let executable = root.join("mysql/bin/mysqld.exe");
        let initialized = Command::new(&executable).args(["--no-defaults", "--initialize-insecure"])
            .arg(format!("--datadir={}", data.display())).creation_flags(0x0800_0000).output().unwrap();
        assert!(initialized.status.success(), "{}", String::from_utf8_lossy(&initialized.stderr));
        let port = crate::install::free_port().unwrap();
        fsx::atomic_write_json(&root.join("Settings/repack.json"), &serde_json::json!({"mysqlPort":port})).unwrap();
        fsx::atomic_write_json(&root.join("Settings/database.json"), &serde_json::json!({"rootPassword":"","appPassword":""})).unwrap();
        let db = crate::db::Db::from_repack(&root, crate::db::Account::Admin).unwrap();
        let start = || {
            let server = Server(Command::new(&executable).args(["--no-defaults", "--bind-address=127.0.0.1", "--mysqlx=0", "--skip-log-bin"])
                .arg(format!("--datadir={}", data.display())).arg(format!("--port={port}"))
                .arg(format!("--log-error={}", root.join("mysql-test.log").display()))
                .stdout(Stdio::null()).stderr(Stdio::null()).creation_flags(0x0800_0000).spawn().unwrap());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !db.ping() { assert!(std::time::Instant::now() < deadline, "{}", fs::read_to_string(root.join("mysql-test.log")).unwrap_or_default()); std::thread::sleep(std::time::Duration::from_millis(200)); }
            server
        };
        let mut server = start();
        db.query("CREATE DATABASE acore_characters; CREATE DATABASE acore_world; CREATE DATABASE acore_auth; CREATE TABLE acore_characters.probe(id INT PRIMARY KEY, note VARCHAR(40)); CREATE TABLE acore_world.probe(id INT NOT NULL DEFAULT 0); INSERT INTO acore_world.probe VALUES(43); CREATE TABLE acore_auth.probe(id INT); INSERT INTO acore_auth.probe VALUES(44); CREATE USER 'fixture_user'@'localhost' IDENTIFIED BY 'fixture'; CREATE TRIGGER acore_characters.probe_insert BEFORE INSERT ON acore_characters.probe FOR EACH ROW SET NEW.note='triggered'; INSERT INTO acore_characters.probe VALUES(42,'original'); CREATE VIEW acore_characters.probe_view AS SELECT c.id,w.id AS world_id FROM acore_characters.probe c CROSS JOIN acore_world.probe w; CREATE PROCEDURE acore_characters.probe_proc() SELECT id FROM acore_characters.probe; CREATE FUNCTION acore_characters.probe_func() RETURNS INT DETERMINISTIC RETURN 73;").unwrap();
        assert_eq!(db.recovery_objects("acore_characters").unwrap().len(), 4);
        crate::schema_check::capture(&db, &root).unwrap();
        let hash = fsx::sha256_file(&root.join(crate::schema_check::CONTRACT)).unwrap();
        db.query("ALTER TABLE acore_world.probe ALTER COLUMN id DROP DEFAULT;").unwrap();
        crate::schema_check::repair_missing_defaults(&db, &root, Some(&hash)).unwrap();
        assert_eq!(db.query("SELECT COLUMN_DEFAULT FROM information_schema.COLUMNS WHERE TABLE_SCHEMA='acore_world' AND TABLE_NAME='probe' AND COLUMN_NAME='id';").unwrap(), "0");
        assert_eq!(db.query("SELECT id FROM acore_world.probe;").unwrap(), "43");
        db.query("ALTER TABLE acore_world.probe ALTER COLUMN id SET DEFAULT 99;").unwrap();
        crate::schema_check::repair_missing_defaults(&db, &root, Some(&hash)).unwrap();
        assert_eq!(db.query("SELECT COLUMN_DEFAULT FROM information_schema.COLUMNS WHERE TABLE_SCHEMA='acore_world' AND TABLE_NAME='probe' AND COLUMN_NAME='id';").unwrap(), "99");
        db.query("SHUTDOWN;").unwrap(); server.0.wait().unwrap();
        let files = inventory(&data).unwrap();
        copy_verified(&data, &point.join("mysql-data"), &files).unwrap();
        let expanded_bytes = files.keys().map(|name| fs::metadata(data.join(name)).unwrap().len()).sum();
        let mut snapshot = Snapshot {path:"mysql-data".into(), files, directories:directories(&data).unwrap(), server_sha256:fsx::sha256_file(&executable).unwrap(), bytes:0, expanded_bytes, archive_sha256:None};
        for directory in &snapshot.directories { fs::create_dir_all(point.join("mysql-data").join(directory)).unwrap(); }
        compress(&data, &point, &mut snapshot).unwrap();
        let mut changed = start();
        db.query("DROP VIEW acore_characters.probe_view; DROP TRIGGER acore_characters.probe_insert; DROP PROCEDURE acore_characters.probe_proc; DROP FUNCTION acore_characters.probe_func; DELETE FROM acore_characters.probe; DROP USER 'fixture_user'@'localhost';").unwrap();
        db.query("SHUTDOWN;").unwrap(); changed.0.wait().unwrap();
        swap(&root, &meta, &point, &snapshot, false).unwrap();
        let mut restored = start();
        assert_eq!(db.recovery_objects("acore_characters").unwrap().len(), 4);
        assert_eq!(db.query("SELECT * FROM acore_characters.probe_view; CALL acore_characters.probe_proc(); SELECT acore_characters.probe_func(); SELECT COUNT(*) FROM mysql.user WHERE user='fixture_user';").unwrap().replace("\r\n", "\n"), "42\t43\n42\n73\n1");
        db.query("INSERT INTO acore_characters.probe VALUES(45,'check');").unwrap();
        assert_eq!(db.query("SELECT note FROM acore_characters.probe WHERE id=45;").unwrap(), "triggered");
        db.query("SHUTDOWN;").unwrap(); restored.0.wait().unwrap();
    }
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, Snapshot) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("server");
        let meta = temp.path().join("metadata");
        let point = temp.path().join("point");
        fsx::atomic_write(&root.join("mysql/bin/mysqld.exe"), b"same-engine").unwrap();
        fsx::atomic_write(&root.join("mysql/data/characters.ibd"), b"live").unwrap();
        fsx::atomic_write(&point.join("mysql-data/characters.ibd"), b"saved").unwrap();
        fs::create_dir_all(point.join("mysql-data/empty_schema")).unwrap();
        let snapshot = Snapshot { path:"mysql-data".into(), files:inventory(&point.join("mysql-data")).unwrap(), directories:directories(&point.join("mysql-data")).unwrap(), server_sha256:fsx::sha256_file(&root.join("mysql/bin/mysqld.exe")).unwrap(), bytes:5, expanded_bytes:5, archive_sha256:None };
        (temp, root, meta, point, snapshot)
    }
    #[test]
    fn interrupted_directory_swap_is_recoverable_and_keeps_previous_bytes() {
        let (_temp, root, meta, point, snapshot) = fixture();
        assert!(swap(&root, &meta, &point, &snapshot, true).is_err());
        assert!(!root.join("mysql/data").exists());
        let old = fs::read_dir(root.join("mysql")).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with("data-before-")).unwrap().path();
        assert_eq!(fs::read(old.join("characters.ibd")).unwrap(), b"live");
        swap(&root, &meta, &point, &snapshot, false).unwrap();
        assert_eq!(fs::read(root.join("mysql/data/characters.ibd")).unwrap(), b"saved");
        assert!(root.join("mysql/data/empty_schema").is_dir());
    }
    #[test]
    fn corrupt_or_different_engine_snapshots_do_not_move_live_data() {
        let (_temp, root, meta, point, snapshot) = fixture();
        fsx::atomic_write(&point.join("mysql-data/characters.ibd"), b"corrupt").unwrap();
        assert!(swap(&root, &meta, &point, &snapshot, false).is_err());
        assert_eq!(fs::read(root.join("mysql/data/characters.ibd")).unwrap(), b"live");
        fsx::atomic_write(&point.join("mysql-data/characters.ibd"), b"saved").unwrap();
        fsx::atomic_write(&root.join("mysql/bin/mysqld.exe"), b"new-engine").unwrap();
        assert!(swap(&root, &meta, &point, &snapshot, false).is_err());
        assert_eq!(fs::read(root.join("mysql/data/characters.ibd")).unwrap(), b"live");
    }
    #[test]
    fn compressed_snapshots_verify_every_file_and_reject_unlisted_entries() {
        let (_temp, root, meta, point, mut snapshot) = fixture();
        let source = point.join("mysql-data");
        compress(&source, &point, &mut snapshot).unwrap();
        assert!(snapshot.archive_sha256.is_some());
        swap(&root, &meta, &point, &snapshot, false).unwrap();
        assert_eq!(fs::read(root.join("mysql/data/characters.ibd")).unwrap(), b"saved");
        fsx::atomic_write(&root.join("mysql/data/characters.ibd"), b"preserve").unwrap();
        let archive_path = point.join(&snapshot.path);
        let encoder = zstd::Encoder::new(fs::File::create(&archive_path).unwrap(), 3).unwrap();
        let mut archive = tar::Builder::new(encoder);
        archive.append_file("unlisted.ibd", &mut fs::File::open(source.join("characters.ibd")).unwrap()).unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        snapshot.archive_sha256 = Some(fsx::sha256_file(&archive_path).unwrap());
        assert!(swap(&root, &meta, &point, &snapshot, false).is_err());
        assert_eq!(fs::read(root.join("mysql/data/characters.ibd")).unwrap(), b"preserve");
        fsx::atomic_write(&archive_path, b"damaged").unwrap();
        assert!(verify(&point, &snapshot).is_err());
    }
    #[test]
    fn private_rehearsals_reject_external_database_connections() {
        let (_temp, root, _meta, _point, _snapshot) = fixture();
        fsx::atomic_write_json(&root.join("Settings/repack.json"), &serde_json::json!({"mysqlPort":3307})).unwrap();
        fsx::atomic_write_json(&root.join("Settings/database.json"), &serde_json::json!({"rootPassword":"test-root","appPassword":"test-app"})).unwrap();
        fsx::atomic_write(&root.join("Settings/worldserver.conf.template"), b"LoginDatabaseInfo = \"@LoginDatabaseInfo@\"\nWorldDatabaseInfo = \"@WorldDatabaseInfo@\"\nCharacterDatabaseInfo = \"@CharacterDatabaseInfo@\"\n").unwrap();
        fsx::atomic_write(&root.join("Settings/authserver.conf.template"), b"LoginDatabaseInfo = \"127.0.0.1;3307;acore;test-app;acore_auth\"\n").unwrap();
        validate_connections(&root, &root).unwrap();
        fsx::atomic_write(&root.join("Settings/authserver.conf.template"), b"LoginDatabaseInfo = \"192.0.2.1;3307;acore;test-app;acore_auth\"\n").unwrap();
        let error = validate_connections(&root, &root).unwrap_err().to_string();
        assert!(error.contains("external database"));
        assert!(!error.contains("test-app"));
    }
    #[test]
    fn a_changed_live_database_invalidates_the_rehearsal() {
        let (_temp, root, _meta, _point, snapshot) = fixture();
        assert!(unchanged(&root, &snapshot).is_err());
        fsx::atomic_write(&root.join("mysql/data/characters.ibd"), b"saved").unwrap();
        fs::create_dir_all(root.join("mysql/data/empty_schema")).unwrap();
        unchanged(&root, &snapshot).unwrap();
        fsx::atomic_write(&root.join("mysql/data/extra.ibd"), b"new data").unwrap();
        assert!(unchanged(&root, &snapshot).is_err());
    }
}
