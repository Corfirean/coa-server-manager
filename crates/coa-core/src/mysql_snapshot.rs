use crate::{fsx, Error, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, BufWriter, Write};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

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
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked {
        return Err(Error::Invalid(
            "Recovery copies cannot follow symbolic links or junctions.".into(),
        ));
    }
    Ok(())
}

fn directories(root: &Path) -> Result<BTreeSet<String>> {
    let canonical = fsx::canonicalize_lenient(root)?;
    let root = canonical.as_path();
    let mut result = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            reject_link(&fs::symlink_metadata(entry.path())?)?;
            let path = fsx::ensure_within(root, &entry.path())?;
            if entry.file_type()?.is_dir() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|e| Error::Invalid(e.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
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
        if target.exists() {
            return Err(Error::Invalid(
                "The recovery destination already exists.".into(),
            ));
        }
        let parent = target
            .parent()
            .ok_or_else(|| Error::Invalid("Invalid recovery destination.".into()))?;
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
            let name = entry
                .path()?
                .to_string_lossy()
                .trim_end_matches('/')
                .to_string();
            if !seen.insert(name.clone()) {
                return Err(Error::Invalid(
                    "Duplicate entry in MySQL recovery archive.".into(),
                ));
            }
            let output = fsx::ensure_within(target, &fsx::safe_join(target, &name)?)?;
            if entry.header().entry_type().is_dir() && snapshot.directories.contains(&name) {
                fs::create_dir_all(output)?;
            } else if entry.header().entry_type().is_file() && snapshot.files.contains_key(&name) {
                expanded = expanded
                    .checked_add(entry.size())
                    .filter(|bytes| *bytes <= snapshot.expanded_bytes)
                    .ok_or_else(|| {
                        Error::Invalid("MySQL recovery archive exceeds its declared size.".into())
                    })?;
                fs::create_dir_all(output.parent().unwrap())?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(output)?;
                if std::io::copy(&mut entry, &mut file)? != entry.size() {
                    return Err(Error::Invalid("Truncated MySQL recovery file.".into()));
                }
                file.flush()?;
            } else {
                return Err(Error::Invalid(
                    "Unexpected entry in MySQL recovery archive.".into(),
                ));
            }
        }
        if expanded != snapshot.expanded_bytes || inventory(target)? != snapshot.files {
            return Err(Error::Invalid(
                "MySQL recovery files failed verification.".into(),
            ));
        }
    } else {
        copy_verified(&point.join(&snapshot.path), target, &snapshot.files)?;
    }
    for directory in &snapshot.directories {
        fs::create_dir_all(fsx::ensure_within(
            target,
            &fsx::safe_join(target, directory)?,
        )?)?;
    }
    if directories(target)? != snapshot.directories {
        return Err(Error::Invalid(
            "The recovery directory inventory differs.".into(),
        ));
    }
    Ok(())
}

fn compress(source: &Path, point: &Path, snapshot: &mut Snapshot) -> Result<()> {
    snapshot.path = "mysql-data.tar.zst".into();
    let path = fsx::safe_join(point, &snapshot.path)?;
    fs::create_dir_all(point)?;
    fsx::require_space(point, snapshot.expanded_bytes)?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let encoder = zstd::Encoder::new(BufWriter::new(file), 3)?;
    let mut archive = tar::Builder::new(encoder);
    for directory in &snapshot.directories {
        archive.append_dir(
            directory,
            fsx::ensure_within(source, &fsx::safe_join(source, directory)?)?,
        )?;
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
    let canonical = fsx::canonicalize_lenient(root)?;
    let root = canonical.as_path();
    let mut files = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            let path = fsx::ensure_within(root, &entry.path())?;
            reject_link(&metadata)?;
            if metadata.is_dir() {
                directories.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(Error::Invalid("Unsupported database snapshot file.".into()));
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|e| Error::Invalid(e.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            fsx::safe_join(root, &relative)?;
            if relative.ends_with(".isl") {
                return Err(Error::Invalid(
                    "External MySQL tablespaces require a separate recovery method.".into(),
                ));
            }
            files.insert(relative, fsx::sha256_file(&path)?);
        }
    }
    Ok(files)
}

pub(crate) fn copy_verified(
    source: &Path,
    target: &Path,
    files: &BTreeMap<String, String>,
) -> Result<()> {
    if target.exists() {
        return Err(Error::Invalid(
            "Database snapshot destination already exists.".into(),
        ));
    }
    let bytes = files.keys().try_fold(0u64, |total, name| -> Result<u64> {
        let path = fsx::ensure_within(source, &fsx::safe_join(source, name)?)?;
        total
            .checked_add(fs::metadata(path)?.len())
            .ok_or_else(|| Error::Invalid("Snapshot size overflow.".into()))
    })?;
    let parent = target
        .parent()
        .ok_or_else(|| Error::Invalid("Invalid snapshot destination.".into()))?;
    fs::create_dir_all(parent)?;
    fsx::require_space(parent, bytes)?;
    fs::create_dir(target)?;
    for (relative, expected) in files {
        let source_file = fsx::ensure_within(source, &fsx::safe_join(source, relative)?)?;
        let target_file = fsx::ensure_within(target, &fsx::safe_join(target, relative)?)?;
        fs::create_dir_all(target_file.parent().unwrap())?;
        fs::copy(&source_file, &target_file)?;
        if fsx::sha256_file(&target_file)? != *expected {
            return Err(Error::Invalid(format!(
                "Database snapshot file {relative} failed verification."
            )));
        }
    }
    if inventory(target)? != *files {
        return Err(Error::Invalid(
            "Database snapshot inventory differs.".into(),
        ));
    }
    Ok(())
}

pub(crate) fn stop(root: &Path) -> Result<()> {
    let observed = crate::process::observe(root, &crate::layout::read_ports(root));
    if [
        observed.mysql.state,
        observed.auth.state,
        observed.world.state,
    ]
    .iter()
    .all(|state| *state == crate::process::ServiceState::Stopped)
        && !crate::multiworld::is_running(root)
    {
        return Ok(());
    }
    let outcome = crate::driver::run(root, crate::driver::Verb::StopAll).map_err(|error| {
        Error::Invalid(format!(
            "Could not stop server {} before copying or restoring MySQL: {error}",
            root.display()
        ))
    })?;
    let status = crate::process::observe(root, &crate::layout::read_ports(root));
    if !outcome.ok
        || [status.mysql.state, status.auth.state, status.world.state]
            .iter()
            .any(|state| *state != crate::process::ServiceState::Stopped)
        || crate::multiworld::is_running(root)
    {
        return Err(Error::Invalid(stop_failure(root, &status, &outcome)));
    }
    Ok(())
}

fn stop_failure(
    root: &Path,
    status: &crate::process::Observed,
    outcome: &crate::driver::DriverOutcome,
) -> String {
    let mut details = vec![format!(
        "The complete server must be stopped before copying or restoring MySQL. Server: {}.",
        root.display()
    )];
    for service in [&status.mysql, &status.auth, &status.world]
        .into_iter()
        .chain(status.secondary_world.iter())
    {
        if service.state != crate::process::ServiceState::Stopped {
            details.push(format!(
                "{}: {:?}; PID: {}; port: {}.",
                service.name,
                service.state,
                service
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "unknown".into()),
                service.port
            ));
        }
        if let Some(conflict) = &service.conflict {
            details.push(format!(
                "Port {} is used by process {} ({}).",
                conflict.port,
                conflict.pid,
                conflict.exe.as_deref().unwrap_or("executable unavailable")
            ));
        }
    }
    let output = crate::diag::redact(&outcome.output);
    let lines: Vec<_> = output.lines().rev().take(12).collect();
    if !lines.is_empty() {
        details.push(lines.into_iter().rev().collect::<Vec<_>>().join("\n"));
    } else if !outcome.ok {
        details.push(format!(
            "The stop command failed (exit code: {:?}).",
            outcome.exit_code
        ));
    }
    details.join("\n")
}

pub(crate) fn stop_previous_rehearsals(meta: &Path) -> Result<()> {
    let location = meta.join("rehearsals");
    if !location.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&location)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() != 32 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let fixture = fsx::ensure_within(&location, &entry.path().join("server"))?;
        let metadata = crate::registry::metadata_dir_for(&fixture)?;
        if !metadata.join("install.json").is_file() {
            continue;
        }
        let (_, installed) = crate::registry::MetaDir::open(&metadata)?;
        if fsx::canonicalize_lenient(Path::new(&installed.server_path))?
            != fsx::canonicalize_lenient(&fixture)?
        {
            return Err(Error::Invalid(
                "The interrupted rehearsal metadata does not match its private server.".into(),
            ));
        }
        stop(&fixture)?;
    }
    Ok(())
}

pub(crate) fn capture(root: &Path, partial: &Path) -> Result<Snapshot> {
    if crate::docker::is_docker(root) || !cfg!(windows) {
        return Err(Error::Invalid(
            "Cold MySQL snapshots require a Windows repack.".into(),
        ));
    }
    crate::backup::with_database(root, |db| {
        if db
            .query("SELECT COUNT(*) FROM performance_schema.replication_connection_configuration;")?
            .trim()
            != "0"
        {
            return Err(Error::Invalid(
                "A replicated MySQL instance requires a separate isolated recovery method.".into(),
            ));
        }
        let data = fsx::ensure_within(root, &root.join("mysql/data"))?;
        for path in db
            .query("SELECT PATH FROM information_schema.INNODB_DATAFILES;")?
            .lines()
        {
            fsx::ensure_within(&data, &data.join(path))?;
        }
        Ok(())
    })?;
    stop(root)?;
    let data = fsx::ensure_within(root, &root.join("mysql/data"))?;
    if !data.join("mysql").is_dir() || !data.join("ibdata1").is_file() {
        return Err(Error::Invalid(
            "The bundled MySQL data directory is incomplete.".into(),
        ));
    }
    let files = inventory(&data)?;
    let bytes = files.keys().try_fold(0u64, |total, file| -> Result<u64> {
        total
            .checked_add(fs::metadata(fsx::safe_join(&data, file)?)?.len())
            .ok_or_else(|| Error::Invalid("Snapshot size overflow.".into()))
    })?;
    let mut snapshot = Snapshot {
        path: "mysql-data".into(),
        files,
        directories: directories(&data)?,
        server_sha256: fsx::sha256_file(&root.join("mysql/bin/mysqld.exe"))?,
        bytes,
        expanded_bytes: bytes,
        archive_sha256: None,
    };
    compress(&data, partial, &mut snapshot)?;
    verify(partial, &snapshot)?;
    Ok(snapshot)
}

pub(crate) fn verify(point: &Path, snapshot: &Snapshot) -> Result<()> {
    if snapshot.files.is_empty() || snapshot.server_sha256.len() != 64 {
        return Err(Error::Invalid(
            "Invalid cold MySQL recovery inventory.".into(),
        ));
    }
    let data = fsx::ensure_within(point, &fsx::safe_join(point, &snapshot.path)?)?;
    if let Some(hash) = &snapshot.archive_sha256 {
        if snapshot.path != "mysql-data.tar.zst"
            || snapshot.expanded_bytes == 0
            || fsx::sha256_file(&data)? != *hash
        {
            return Err(Error::Invalid(
                "The compressed MySQL recovery copy is damaged.".into(),
            ));
        }
        for name in snapshot.files.keys().chain(snapshot.directories.iter()) {
            fsx::safe_join(point, name)?;
        }
        return Ok(());
    }
    if snapshot.path != "mysql-data" {
        return Err(Error::Invalid("Unsupported MySQL recovery format.".into()));
    }
    if inventory(&data)? != snapshot.files {
        return Err(Error::Invalid(
            "The cold MySQL recovery copy is damaged or incomplete.".into(),
        ));
    }
    if directories(&data)? != snapshot.directories {
        return Err(Error::Invalid(
            "The cold MySQL recovery directories are incomplete.".into(),
        ));
    }
    Ok(())
}

// Keep the replaced data directory. If interrupted between renames, a retry can install a new verified copy.
pub(crate) fn swap(
    root: &Path,
    meta: &Path,
    point: &Path,
    snapshot: &Snapshot,
    fail_after_move: bool,
) -> Result<PathBuf> {
    verify(point, snapshot)?;
    if fsx::sha256_file(&root.join("mysql/bin/mysqld.exe"))? != snapshot.server_sha256 {
        return Err(Error::Invalid(
            "The MySQL executable differs from the recovery point.".into(),
        ));
    }
    let mysql = fsx::ensure_within(root, &root.join("mysql"))?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let staging = fsx::ensure_within(&mysql, &mysql.join(format!("data-restore-{token}")))?;
    let previous = fsx::ensure_within(&mysql, &mysql.join(format!("data-before-restore-{token}")))?;
    let live = fsx::ensure_within(&mysql, &mysql.join("data"))?;
    copy_snapshot(point, snapshot, &staging)?;
    let journal = meta
        .join("diagnostics")
        .join(format!("database-restore-{token}.json"));
    fsx::atomic_write_json(
        &journal,
        &serde_json::json!({"schema":1,"state":"prepared","previous":previous,"staging":staging}),
    )?;
    if live.exists() {
        fs::rename(&live, &previous)?;
    }
    if fail_after_move {
        return Err(Error::Invalid(
            "Simulated interruption after saving the previous MySQL directory.".into(),
        ));
    }
    fs::rename(&staging, &live)?;
    fsx::atomic_write_json(
        &journal,
        &serde_json::json!({"schema":1,"state":"restored","previous":previous}),
    )?;
    Ok(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_stop_identifies_the_server_process_and_retains_redacted_launcher_reason() {
        use crate::process::{Observed, ServiceState, ServiceStatus};
        let stopped = |name| ServiceStatus {
            name,
            state: ServiceState::Stopped,
            pid: None,
            port: 0,
            port_ready: false,
            conflict: None,
            uptime_secs: None,
        };
        let status = Observed {
            mysql: ServiceStatus {
                state: ServiceState::Stopping,
                pid: Some(42),
                port: 3307,
                ..stopped("mysql")
            },
            auth: stopped("auth"),
            world: stopped("world"),
            secondary_world: None,
        };
        let outcome = crate::driver::DriverOutcome {
            ok: false,
            exit_code: Some(1),
            code: None,
            human: None,
            output:
                "MySQL is still stopping; wait before copying the repack.\nrootPassword=hunter2"
                    .into(),
        };
        let message = stop_failure(Path::new("private-rehearsal/server"), &status, &outcome);
        assert!(
            message.contains("private-rehearsal")
                && message.contains("PID: 42")
                && message.contains("3307")
        );
        assert!(message.contains("MySQL is still stopping"));
        assert!(!message.contains("hunter2"));
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "requires COA_MYSQL_TEST_BIN pointing to a repack with Runtime/python and Scripts/manage.py"]
    fn real_shutdown_with_broken_templates_preserves_database_rows() {
        use std::os::windows::process::CommandExt;
        use std::process::{Child, Command, Stdio};
        struct Server(Child);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let bin = PathBuf::from(std::env::var("COA_MYSQL_TEST_BIN").unwrap());
        let source = bin.parent().unwrap().parent().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("shutdown-fixture");
        copy_verified(&bin, &root.join("mysql/bin"), &inventory(&bin).unwrap()).unwrap();
        copy_verified(
            &source.join("Runtime/python"),
            &root.join("Runtime/python"),
            &inventory(&source.join("Runtime/python")).unwrap(),
        )
        .unwrap();
        fsx::atomic_write(
            &root.join("Scripts/manage.py"),
            &fs::read(source.join("Scripts/manage.py")).unwrap(),
        )
        .unwrap();
        let data = root.join("mysql/data");
        let executable = root.join("mysql/bin/mysqld.exe");
        let initialized = Command::new(&executable)
            .args(["--no-defaults", "--initialize-insecure"])
            .arg(format!("--datadir={}", data.display()))
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            initialized.status.success(),
            "{}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        let sockets: Vec<_> = (0..4)
            .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
            .collect();
        let ports: Vec<_> = sockets
            .iter()
            .map(|socket| socket.local_addr().unwrap().port())
            .collect();
        fsx::atomic_write_json(&root.join("Settings/repack.json"), &serde_json::json!({"mysqlPort":ports[0],"authPort":ports[1],"worldPort":ports[2],"raPort":ports[3]})).unwrap();
        drop(sockets);
        fsx::atomic_write_json(
            &root.join("Settings/database.json"),
            &serde_json::json!({"rootPassword":"","appPassword":""}),
        )
        .unwrap();
        fsx::atomic_write(
            &root.join("mysql/admin-client.ini"),
            format!(
                "[client]\nuser=root\nhost=127.0.0.1\nport={}\nprotocol=tcp\n",
                ports[0]
            )
            .as_bytes(),
        )
        .unwrap();
        fsx::atomic_write(
            &root.join("Settings/worldserver.conf.template"),
            b"WorldDatabaseInfo = @UNKNOWN_DATABASE@\n",
        )
        .unwrap();
        let original = fs::read(root.join("Settings/worldserver.conf.template")).unwrap();
        let db = crate::db::Db::from_repack(&root, crate::db::Account::Admin).unwrap();
        let start = || {
            let server = Server(
                Command::new(&executable)
                    .args([
                        "--no-defaults",
                        "--bind-address=127.0.0.1",
                        "--mysqlx=0",
                        "--skip-log-bin",
                    ])
                    .arg(format!("--datadir={}", data.display()))
                    .arg(format!("--port={}", ports[0]))
                    .arg(format!(
                        "--log-error={}",
                        root.join("mysql-test.log").display()
                    ))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .unwrap(),
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !db.ping() {
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            fsx::atomic_write_json(
                &root.join(".state/mysql.json"),
                &crate::process::identity(server.0.id()).unwrap(),
            )
            .unwrap();
            server
        };
        let mut server = start();
        db.query("CREATE DATABASE acore_characters; CREATE TABLE acore_characters.shutdown_probe(id INT PRIMARY KEY); INSERT INTO acore_characters.shutdown_probe VALUES(73);").unwrap();
        db.query("CREATE DATABASE acore_auth; CREATE DATABASE acore_world; CREATE VIEW acore_characters.shutdown_view AS SELECT id FROM acore_characters.shutdown_probe;").unwrap();
        let copied_configuration = serde_json::json!({"root":temp.path().join("original-installation"),"ports":{"mysqlPort":ports[0],"authPort":ports[1],"worldPort":ports[2],"raPort":ports[3]}});
        fsx::atomic_write_json(
            &root.join(".state/configuration.json"),
            &copied_configuration,
        )
        .unwrap();
        let meta = crate::registry::metadata_dir_for(&root).unwrap();
        crate::registry::MetaDir::create(
            &root,
            &crate::registry::InstallMeta::new(crate::registry::InstallKind::New, &root),
        )
        .unwrap();
        let point = crate::backup::create(
            &root,
            &meta,
            crate::backup::Kind::Full,
            crate::backup::Trigger::BeforeRepair,
            None,
            &|_| {},
        )
        .unwrap();
        assert!(point.mysql_snapshot.is_some());
        assert!(crate::backup::verify(&meta, &point.id).unwrap().ok);
        crate::update::RepackEnv {
            root: &root,
            meta_dir: &meta,
        }
        .verify_recovery_point(&point)
        .unwrap();
        assert!(server.0.wait().unwrap().success());
        assert_eq!(
            fs::read(root.join("Settings/worldserver.conf.template")).unwrap(),
            original
        );
        assert_eq!(
            fsx::read_json::<serde_json::Value>(&root.join(".state/configuration.json")).unwrap(),
            copied_configuration
        );
        fs::remove_file(root.join(".state/configuration.json")).unwrap();
        let mut restarted = start();
        assert_eq!(
            db.query("SELECT id FROM acore_characters.shutdown_probe;")
                .unwrap(),
            "73"
        );
        fsx::atomic_write_json(
            &root.join(".state/mysql.json"),
            &crate::process::identity(std::process::id()).unwrap(),
        )
        .unwrap();
        assert!(stop(&root)
            .unwrap_err()
            .to_string()
            .contains("different live process"));
        assert_eq!(
            db.query("SELECT id FROM acore_characters.shutdown_probe;")
                .unwrap(),
            "73"
        );
        fs::remove_file(root.join("Settings/worldserver.conf.template")).unwrap();
        fs::remove_file(root.join(".state/mysql.json")).unwrap();
        let observed = crate::process::observe(&root, &crate::layout::read_ports(&root));
        assert_eq!(observed.mysql.state, crate::process::ServiceState::Running);
        assert!(observed.mysql.port_ready);
        assert!(crate::process::listeners().contains(&(ports[0], observed.mysql.pid.unwrap())));
        stop(&root).unwrap();
        assert!(restarted.0.wait().unwrap().success());
        let mut third = start();
        assert_eq!(
            db.query("SELECT id FROM acore_characters.shutdown_view;")
                .unwrap(),
            "73"
        );
        db.query("UPDATE acore_characters.shutdown_probe SET id=74;")
            .unwrap();
        fsx::atomic_write(&root.join(".state/mysql.json"), b"{truncated").unwrap();
        stop(&root).unwrap();
        assert!(third.0.wait().unwrap().success());
        assert_eq!(
            fs::read(root.join(".state/mysql-before-shutdown-recovery.json")).unwrap(),
            b"{truncated"
        );
        let point_dir = crate::backup::point_json(&meta, &point.id)
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let retained = swap(
            &root,
            &meta,
            &point_dir,
            point.mysql_snapshot.as_ref().unwrap(),
            false,
        )
        .unwrap();
        assert!(retained.is_dir());
        let mut recovered = start();
        assert_eq!(
            db.query("SELECT id FROM acore_characters.shutdown_view;")
                .unwrap(),
            "73"
        );
        stop(&root).unwrap();
        assert!(recovered.0.wait().unwrap().success());
        // Exercise the public Repair pipeline with real signed packages and SQL,
        // rather than only the snapshot and journal helpers.
        let mut package_server = start();
        db.query("CREATE TABLE acore_auth.account(id INT PRIMARY KEY, username VARCHAR(40)); INSERT INTO acore_auth.account VALUES(73,'FIXTURE'); CREATE TABLE acore_auth.realmlist(id INT PRIMARY KEY, name VARCHAR(40), address VARCHAR(40), localAddress VARCHAR(40), port INT, flag INT); INSERT INTO acore_auth.realmlist VALUES(1,'AzerothCore','127.0.0.1','127.0.0.1',18085,0); CREATE TABLE acore_characters.characters(guid INT PRIMARY KEY, account INT, name VARCHAR(40), race INT, class INT); INSERT INTO acore_characters.characters VALUES(73,73,'Fixture',7,14); CREATE TABLE acore_world.playercreateinfo(race INT, class INT, PRIMARY KEY(race,class));").unwrap();
        let starts = [1, 2, 3, 4, 5, 6, 7, 8, 10, 11]
            .into_iter()
            .flat_map(|race| (12..=32).map(move |class| format!("({race},{class})")))
            .collect::<Vec<_>>()
            .join(",");
        db.query(&format!(
            "INSERT INTO acore_world.playercreateinfo VALUES {starts};"
        ))
        .unwrap();
        let password = "a".repeat(48);
        db.query(&format!(
            "ALTER USER 'root'@'localhost' IDENTIFIED BY '{password}';"
        ))
        .unwrap();
        fsx::atomic_write_json(
            &root.join("Settings/database.json"),
            &serde_json::json!({"rootPassword":password,"appPassword":"b".repeat(48)}),
        )
        .unwrap();
        fsx::atomic_write(&root.join("Settings/worldserver.conf.template"), b"WorldDatabaseInfo = @WorldDatabaseInfo@\nCharacterDatabaseInfo = @CharacterDatabaseInfo@\nLoginDatabaseInfo = @LoginDatabaseInfo@\n").unwrap();
        fsx::atomic_write(
            &root.join("Settings/authserver.conf.template"),
            b"LoginDatabaseInfo = @LoginDatabaseInfo@\n",
        )
        .unwrap();
        fsx::atomic_write(
            &root.join("Settings/mod_ascension_compat.conf.template"),
            b"Ascension.DbcPath = @ASCENSION_DBC@\n",
        )
        .unwrap();
        let startup = crate::driver::run(&root, crate::driver::Verb::StartMysql).unwrap();
        assert!(startup.ok, "{}", crate::diag::redact(&startup.output));
        let db = crate::db::Db::from_repack(&root, crate::db::Account::Admin).unwrap();
        crate::schema_check::capture(&db, &root).unwrap();
        assert!(crate::schema_check::check(&db, &root).unwrap().is_empty());
        stop(&root).unwrap();
        assert!(package_server.0.wait().unwrap().success());
        struct StopFixture(PathBuf);
        impl Drop for StopFixture {
            fn drop(&mut self) {
                let _ = stop(&self.0);
            }
        }
        let _stop_fixture = StopFixture(root.clone());
        use base64::{engine::general_purpose::STANDARD, Engine};
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[7; 32]);
        let trusted = STANDARD.encode(key.verifying_key().to_bytes());
        let source = temp.path().join("repair-package-source");
        fsx::atomic_write(&source.join("Core/reference/repair-probe.dll"), b"official").unwrap();
        fsx::atomic_write(
            &source.join(crate::schema_check::CONTRACT),
            &fs::read(root.join(crate::schema_check::CONTRACT)).unwrap(),
        )
        .unwrap();
        let build = |kind, version: &str, destination: &Path, migrations| {
            let manifest = crate::package::build(
                &source,
                destination,
                &crate::package::BuildOptions {
                    kind,
                    version: version.into(),
                    core_commit: None,
                    built_at: "fixture".into(),
                    part_size: 1024 * 1024,
                    bots_commit: None,
                    migrations,
                },
                &|_| {},
            )
            .unwrap();
            let bytes = fs::read(destination.join("manifest.json")).unwrap();
            fsx::atomic_write(
                &destination.join("manifest.json.sig"),
                STANDARD.encode(key.sign(&bytes).to_bytes()).as_bytes(),
            )
            .unwrap();
            manifest
        };
        let base = temp.path().join("repair-base");
        let base_manifest = build(crate::manifest::Kind::Base, "1.0.0", &base, vec![]);
        let update = temp.path().join("repair-update");
        let update_manifest = build(crate::manifest::Kind::Update, "2.0.0", &update, vec![]);
        fsx::atomic_write_json(&meta.join("manifests/base.json"), &base_manifest).unwrap();
        fsx::atomic_write_json(&meta.join("manifests/update-2.0.0.json"), &update_manifest)
            .unwrap();
        let (_, mut installed) = crate::registry::MetaDir::open(&meta).unwrap();
        installed.core.version = Some("2.0.0".into());
        fsx::atomic_write_json(&meta.join("install.json"), &installed).unwrap();
        fsx::atomic_write(&root.join("Core/reference/repair-probe.dll"), b"broken").unwrap();
        let base_source = crate::pkgsource::Source::Dir(base);
        let update_source = crate::pkgsource::Source::Dir(update);
        let report = crate::repair::run(
            &root,
            &meta,
            &base_source,
            &update_source,
            &trusted,
            &|_, _| {},
        )
        .unwrap();
        assert!(report.error.is_none());
        assert!(report
            .database
            .iter()
            .all(|check| check.problems.is_empty()));
        assert_eq!(
            fs::read(root.join("Core/reference/repair-probe.dll")).unwrap(),
            b"official"
        );
        crate::update::ensure_recovered(&meta).unwrap();
        fsx::atomic_write(
            &root.join("Core/reference/repair-probe.dll"),
            b"before verification failure",
        )
        .unwrap();
        let error = crate::repair::run(
            &root,
            &meta,
            &base_source,
            &update_source,
            &trusted,
            &|phase, _| {
                if phase == "Checking database after repair" {
                    fs::remove_file(root.join("Core/reference/repair-probe.dll")).unwrap();
                }
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("repair"));
        assert_eq!(
            fs::read(root.join("Core/reference/repair-probe.dll")).unwrap(),
            b"before verification failure"
        );
        crate::update::ensure_recovered(&meta).unwrap();
        let failed_sql =
            "UPDATE shutdown_probe SET id=75; CREATE TABLE repair_partial(id INT); INVALID SQL;";
        fsx::atomic_write(
            &source.join("_migrations/characters/repair-failure.sql"),
            failed_sql.as_bytes(),
        )
        .unwrap();
        let failing = temp.path().join("repair-failing-update");
        let manifest = build(
            crate::manifest::Kind::Update,
            "2.0.0",
            &failing,
            vec![crate::manifest::Migration {
                id: "repair-failure".into(),
                db: "characters".into(),
                sha256: fsx::sha256_bytes(failed_sql.as_bytes()),
                compatible_sha256: vec![],
                destructive: false,
            }],
        );
        fsx::atomic_write_json(&meta.join("manifests/update-2.0.0.json"), &manifest).unwrap();
        fsx::atomic_write(
            &root.join("Core/reference/repair-probe.dll"),
            b"before failed repair",
        )
        .unwrap();
        let report = crate::repair::run(
            &root,
            &meta,
            &base_source,
            &crate::pkgsource::Source::Dir(failing),
            &trusted,
            &|_, _| {},
        )
        .unwrap();
        assert!(report.error.is_some());
        assert_eq!(
            fs::read(root.join("Core/reference/repair-probe.dll")).unwrap(),
            b"before failed repair"
        );
        crate::update::ensure_recovered(&meta).unwrap();
        crate::backup::with_database(&root, |db| {
            assert_eq!(db.query("SELECT id FROM acore_characters.shutdown_view;")?, "73");
            assert_eq!(db.query("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA='acore_characters' AND TABLE_NAME='repair_partial';")?, "0");
            assert_eq!(db.query("SELECT username FROM acore_auth.account WHERE id=73;")?, "FIXTURE");
            Ok(())
        }).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires COA_MYSQL_TEST_BIN pointing to the bundled MySQL tools"]
    fn real_mysql_restores_objects_users_rows_and_missing_defaults() {
        use std::os::windows::process::CommandExt;
        use std::process::{Child, Command, Stdio};
        struct Server(Child);
        impl Drop for Server {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let bin = PathBuf::from(std::env::var("COA_MYSQL_TEST_BIN").unwrap());
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("server");
        let meta = temp.path().join("metadata");
        let point = temp.path().join("point");
        copy_verified(&bin, &root.join("mysql/bin"), &inventory(&bin).unwrap()).unwrap();
        let data = root.join("mysql/data");
        let executable = root.join("mysql/bin/mysqld.exe");
        let initialized = Command::new(&executable)
            .args(["--no-defaults", "--initialize-insecure"])
            .arg(format!("--datadir={}", data.display()))
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            initialized.status.success(),
            "{}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        let port = crate::install::free_port().unwrap();
        fsx::atomic_write_json(
            &root.join("Settings/repack.json"),
            &serde_json::json!({"mysqlPort":port}),
        )
        .unwrap();
        fsx::atomic_write_json(
            &root.join("Settings/database.json"),
            &serde_json::json!({"rootPassword":"","appPassword":""}),
        )
        .unwrap();
        let db = crate::db::Db::from_repack(&root, crate::db::Account::Admin).unwrap();
        let start = || {
            let server = Server(
                Command::new(&executable)
                    .args([
                        "--no-defaults",
                        "--bind-address=127.0.0.1",
                        "--mysqlx=0",
                        "--skip-log-bin",
                    ])
                    .arg(format!("--datadir={}", data.display()))
                    .arg(format!("--port={port}"))
                    .arg(format!(
                        "--log-error={}",
                        root.join("mysql-test.log").display()
                    ))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .creation_flags(0x0800_0000)
                    .spawn()
                    .unwrap(),
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while !db.ping() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "{}",
                    fs::read_to_string(root.join("mysql-test.log")).unwrap_or_default()
                );
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            server
        };
        let mut server = start();
        db.query("CREATE DATABASE acore_characters; CREATE DATABASE acore_world; CREATE DATABASE acore_auth; CREATE TABLE acore_characters.probe(id INT PRIMARY KEY, note VARCHAR(40)); CREATE TABLE acore_world.probe(id INT NOT NULL DEFAULT 0); INSERT INTO acore_world.probe VALUES(43); CREATE TABLE acore_auth.probe(id INT); INSERT INTO acore_auth.probe VALUES(44); CREATE USER 'fixture_user'@'localhost' IDENTIFIED BY 'fixture'; CREATE TRIGGER acore_characters.probe_insert BEFORE INSERT ON acore_characters.probe FOR EACH ROW SET NEW.note='triggered'; INSERT INTO acore_characters.probe VALUES(42,'original'); CREATE VIEW acore_characters.probe_view AS SELECT c.id,w.id AS world_id FROM acore_characters.probe c CROSS JOIN acore_world.probe w; CREATE PROCEDURE acore_characters.probe_proc() SELECT id FROM acore_characters.probe; CREATE FUNCTION acore_characters.probe_func() RETURNS INT DETERMINISTIC RETURN 73;").unwrap();
        // Shared auth must not run again when the active realm changes.
        use crate::migrations::Store;
        db.query("CREATE DATABASE acore_world_wildcard;").unwrap();
        let migration_dir = root.join("migration-fixture");
        fs::create_dir_all(migration_dir.join("auth")).unwrap();
        fs::create_dir_all(migration_dir.join("world")).unwrap();
        let auth_sql = b"INSERT INTO acore_auth.probe VALUES(45);";
        fs::write(migration_dir.join("auth/shared_once.sql"), auth_sql).unwrap();
        let auth_list = vec![crate::manifest::Migration {
            id: "shared_once".into(),
            db: "auth".into(),
            sha256: fsx::sha256_bytes(auth_sql),
            destructive: false,
            compatible_sha256: vec![],
        }];
        assert_eq!(
            crate::migrations::apply_pending(&db, &auth_list, &migration_dir, &|| Ok(
                "fixture".into()
            ))
            .unwrap()
            .applied
            .len(),
            1
        );
        let wildcard = db.clone().for_realm(crate::realms::Mode::Wildcard);
        assert_eq!(
            crate::migrations::pending_count(&wildcard, &auth_list).unwrap(),
            0,
            "auth is shared by both realms"
        );
        assert!(
            crate::migrations::apply_pending(&wildcard, &auth_list, &migration_dir, &|| Ok(
                "fixture".into()
            ))
            .unwrap()
            .applied
            .is_empty()
        );
        assert_eq!(
            db.query("SELECT COUNT(*) FROM acore_auth.probe WHERE id=45;")
                .unwrap(),
            "1"
        );
        let mut failed = crate::migrations::LedgerRow {
            db: "auth".into(),
            id: "shared_once".into(),
            sha256: auth_list[0].sha256.clone(),
            status: crate::migrations::Status::Failed,
            error: Some("interrupted fixture".into()),
            baseline: false,
        };
        db.put(&failed).unwrap();
        assert!(crate::migrations::pending_count(&wildcard, &auth_list).is_err());
        failed.status = crate::migrations::Status::Applied;
        failed.error = None;
        db.put(&failed).unwrap();

        // Legacy per-world auth records are adopted without replaying their SQL.
        let legacy_sql = b"INSERT INTO acore_auth.probe VALUES(46);";
        fs::write(migration_dir.join("auth/legacy_auth.sql"), legacy_sql).unwrap();
        db.query(&format!("INSERT INTO acore_auth.probe VALUES(46); REPLACE INTO acore_world.coa_manager_migrations VALUES('auth','legacy_auth','{}','applied',NOW(),NULL,0);", fsx::sha256_bytes(legacy_sql))).unwrap();
        let legacy_auth = vec![crate::manifest::Migration {
            id: "legacy_auth".into(),
            db: "auth".into(),
            sha256: fsx::sha256_bytes(legacy_sql),
            destructive: false,
            compatible_sha256: vec![],
        }];
        assert_eq!(
            crate::migrations::pending_count(&wildcard, &legacy_auth).unwrap(),
            0
        );
        assert!(
            crate::migrations::apply_pending(&wildcard, &legacy_auth, &migration_dir, &|| Ok(
                "fixture".into()
            ))
            .unwrap()
            .applied
            .is_empty()
        );
        assert_eq!(
            db.query("SELECT COUNT(*) FROM acore_auth.probe WHERE id=46;")
                .unwrap(),
            "1"
        );

        // A core history cleanup cannot make previously recorded SQL pending again.
        db.query("CREATE TABLE acore_world.updates(name VARCHAR(150) PRIMARY KEY) DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci; INSERT INTO acore_world.updates VALUES('LEGACY_WORLD.sql'); INSERT INTO acore_world.probe VALUES(99);").unwrap();
        db.load().unwrap();
        db.query("DROP TABLE acore_world.updates;").unwrap();
        let legacy_world_sql = b"INSERT INTO acore_world.probe VALUES(99);";
        fs::write(
            migration_dir.join("world/legacy_world.sql"),
            legacy_world_sql,
        )
        .unwrap();
        let legacy_world = vec![crate::manifest::Migration {
            id: "legacy_world".into(),
            db: "world".into(),
            sha256: fsx::sha256_bytes(legacy_world_sql),
            destructive: false,
            compatible_sha256: vec![],
        }];
        assert_eq!(
            crate::migrations::pending_count(&db, &legacy_world).unwrap(),
            0
        );
        assert!(
            crate::migrations::apply_pending(&db, &legacy_world, &migration_dir, &|| Ok(
                "fixture".into()
            ))
            .unwrap()
            .applied
            .is_empty()
        );
        assert_eq!(
            db.query("SELECT COUNT(*) FROM acore_world.probe WHERE id=99;")
                .unwrap(),
            "1"
        );
        db.query("DELETE FROM acore_world.probe WHERE id=99;")
            .unwrap();
        assert_eq!(db.recovery_objects("acore_characters").unwrap().len(), 4);
        db.query("CREATE TABLE acore_characters.coa_squid_pending(path VARCHAR(255), sha256 CHAR(64)); INSERT INTO acore_characters.coa_squid_pending VALUES('playerbots/base/ai_playerbot_texts.sql', REPEAT('a',64));").unwrap();
        let squid_problems =
            crate::squid::database_problems(&root, crate::realms::Mode::Coa, &|sql| {
                assert!(sql.starts_with("SELECT "));
                db.query(sql)
            })
            .unwrap();
        assert_eq!(squid_problems.len(), 1);
        assert!(squid_problems[0].detail.contains("interrupted"));
        assert_eq!(
            db.query("SELECT COUNT(*) FROM acore_characters.coa_squid_pending;")
                .unwrap(),
            "1"
        );
        crate::schema_check::capture(&db, &root).unwrap();
        let hash = fsx::sha256_file(&root.join(crate::schema_check::CONTRACT)).unwrap();
        db.query("ALTER TABLE acore_world.probe ALTER COLUMN id DROP DEFAULT;")
            .unwrap();
        crate::schema_check::repair_missing_defaults(&db, &root, Some(&hash)).unwrap();
        assert_eq!(db.query("SELECT COLUMN_DEFAULT FROM information_schema.COLUMNS WHERE TABLE_SCHEMA='acore_world' AND TABLE_NAME='probe' AND COLUMN_NAME='id';").unwrap(), "0");
        assert_eq!(db.query("SELECT id FROM acore_world.probe;").unwrap(), "43");
        db.query("ALTER TABLE acore_world.probe ALTER COLUMN id SET DEFAULT 99;")
            .unwrap();
        crate::schema_check::repair_missing_defaults(&db, &root, Some(&hash)).unwrap();
        assert_eq!(db.query("SELECT COLUMN_DEFAULT FROM information_schema.COLUMNS WHERE TABLE_SCHEMA='acore_world' AND TABLE_NAME='probe' AND COLUMN_NAME='id';").unwrap(), "99");
        db.query("SHUTDOWN;").unwrap();
        server.0.wait().unwrap();
        let files = inventory(&data).unwrap();
        copy_verified(&data, &point.join("mysql-data"), &files).unwrap();
        let expanded_bytes = files
            .keys()
            .map(|name| fs::metadata(data.join(name)).unwrap().len())
            .sum();
        let mut snapshot = Snapshot {
            path: "mysql-data".into(),
            files,
            directories: directories(&data).unwrap(),
            server_sha256: fsx::sha256_file(&executable).unwrap(),
            bytes: 0,
            expanded_bytes,
            archive_sha256: None,
        };
        for directory in &snapshot.directories {
            fs::create_dir_all(point.join("mysql-data").join(directory)).unwrap();
        }
        compress(&data, &point, &mut snapshot).unwrap();
        let mut changed = start();
        db.query("DROP VIEW acore_characters.probe_view; DROP TRIGGER acore_characters.probe_insert; DROP PROCEDURE acore_characters.probe_proc; DROP FUNCTION acore_characters.probe_func; DELETE FROM acore_characters.probe; DROP USER 'fixture_user'@'localhost';").unwrap();
        db.query("SHUTDOWN;").unwrap();
        changed.0.wait().unwrap();
        swap(&root, &meta, &point, &snapshot, false).unwrap();
        let mut restored = start();
        assert_eq!(db.recovery_objects("acore_characters").unwrap().len(), 4);
        assert_eq!(db.query("SELECT * FROM acore_characters.probe_view; CALL acore_characters.probe_proc(); SELECT acore_characters.probe_func(); SELECT COUNT(*) FROM mysql.user WHERE user='fixture_user';").unwrap().replace("\r\n", "\n"), "42\t43\n42\n73\n1");
        db.query("INSERT INTO acore_characters.probe VALUES(45,'check');")
            .unwrap();
        assert_eq!(
            db.query("SELECT note FROM acore_characters.probe WHERE id=45;")
                .unwrap(),
            "triggered"
        );
        db.query("SHUTDOWN;").unwrap();
        restored.0.wait().unwrap();
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
        let snapshot = Snapshot {
            path: "mysql-data".into(),
            files: inventory(&point.join("mysql-data")).unwrap(),
            directories: directories(&point.join("mysql-data")).unwrap(),
            server_sha256: fsx::sha256_file(&root.join("mysql/bin/mysqld.exe")).unwrap(),
            bytes: 5,
            expanded_bytes: 5,
            archive_sha256: None,
        };
        (temp, root, meta, point, snapshot)
    }
    #[test]
    fn interrupted_directory_swap_is_recoverable_and_keeps_previous_bytes() {
        let (_temp, root, meta, point, snapshot) = fixture();
        assert!(swap(&root, &meta, &point, &snapshot, true).is_err());
        assert!(!root.join("mysql/data").exists());
        let old = fs::read_dir(root.join("mysql"))
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("data-before-"))
            .unwrap()
            .path();
        assert_eq!(fs::read(old.join("characters.ibd")).unwrap(), b"live");
        swap(&root, &meta, &point, &snapshot, false).unwrap();
        assert_eq!(
            fs::read(root.join("mysql/data/characters.ibd")).unwrap(),
            b"saved"
        );
        assert!(root.join("mysql/data/empty_schema").is_dir());
    }
    #[test]
    fn corrupt_or_different_engine_snapshots_do_not_move_live_data() {
        let (_temp, root, meta, point, snapshot) = fixture();
        fsx::atomic_write(&point.join("mysql-data/characters.ibd"), b"corrupt").unwrap();
        assert!(swap(&root, &meta, &point, &snapshot, false).is_err());
        assert_eq!(
            fs::read(root.join("mysql/data/characters.ibd")).unwrap(),
            b"live"
        );
        fsx::atomic_write(&point.join("mysql-data/characters.ibd"), b"saved").unwrap();
        fsx::atomic_write(&root.join("mysql/bin/mysqld.exe"), b"new-engine").unwrap();
        assert!(swap(&root, &meta, &point, &snapshot, false).is_err());
        assert_eq!(
            fs::read(root.join("mysql/data/characters.ibd")).unwrap(),
            b"live"
        );
    }
    #[test]
    fn compressed_snapshots_verify_every_file_and_reject_unlisted_entries() {
        let (_temp, root, meta, point, mut snapshot) = fixture();
        let source = point.join("mysql-data");
        compress(&source, &point, &mut snapshot).unwrap();
        assert!(snapshot.archive_sha256.is_some());
        swap(&root, &meta, &point, &snapshot, false).unwrap();
        assert_eq!(
            fs::read(root.join("mysql/data/characters.ibd")).unwrap(),
            b"saved"
        );
        fsx::atomic_write(&root.join("mysql/data/characters.ibd"), b"preserve").unwrap();
        let archive_path = point.join(&snapshot.path);
        let encoder = zstd::Encoder::new(fs::File::create(&archive_path).unwrap(), 3).unwrap();
        let mut archive = tar::Builder::new(encoder);
        archive
            .append_file(
                "unlisted.ibd",
                &mut fs::File::open(source.join("characters.ibd")).unwrap(),
            )
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        snapshot.archive_sha256 = Some(fsx::sha256_file(&archive_path).unwrap());
        assert!(swap(&root, &meta, &point, &snapshot, false).is_err());
        assert_eq!(
            fs::read(root.join("mysql/data/characters.ibd")).unwrap(),
            b"preserve"
        );
        fsx::atomic_write(&archive_path, b"damaged").unwrap();
        assert!(verify(&point, &snapshot).is_err());
    }
}
