//! Installing a new server as a Docker installation, from a Linux package.
//!
//! The order of events is the one of `install::install_base`: nothing appears at the destination until the package is
//! signed-off, downloaded, verified, extracted and its database built, all of it in a sibling `<dest>.installing`
//! folder. But this is code of its own: the Windows installer is not touched and nothing here is specific to the
//! Windows layout. What both installers use is platform-neutral: the signed manifest, the download, the extraction,
//! the migration ledger and the registry.
//!
//! The databases start from the package's own dumps (`Database/baseline/<kind>.sql.zst`, the format of the Manager's
//! backups), as a Windows package starts from its prepared database: they hold the state in which every migration of the
//! package is already applied, and the installer records them as such. They cannot be rebuilt from the repository's
//! SQL files: some migrations are guards that only apply to the maintainers' own database. The game data is not part
//! of the package: the person points at a folder they already have.

use std::fs;
use std::path::{Path, PathBuf};

use super::cli::{Docker, SystemDocker};
use super::{Config, MARKER};
use crate::db::{self, Account, Db};
use crate::download::Cancel;
use crate::driver::Verb;
use crate::error::{Error, Result};
use crate::fsx;
use crate::install::{free_port, problem, random_hex, Installed, Preflight, Step};
use crate::manifest::{self, Migration};
use crate::migrations;
use crate::package::BASELINE_DIR;
use crate::pkgsource::{fetch_manifest, fetch_parts, Source};
use crate::registry::{metadata_dir_for, InstallKind, InstallMeta, MetaDir, Registry, LAYOUT_DOCKER_V1};

/// Created in the staging folder so a leftover of an earlier attempt can be told from somebody else's folder.
const STAGING_MARKER: &str = ".coa-installing";
/// The three game databases: kind (the name of the dump), schema.
const DATABASES: [(&str, &str); 3] = [("auth", "acore_auth"), ("characters", "acore_characters"), ("world", "acore_world")];
/// Ports the game uses by default on this computer.
const DEFAULT_PORTS: (u16, u16, u16) = (3724, 8085, 3443);

pub struct Params<'a> {
    pub source: Source,
    pub dest: PathBuf,
    /// A folder with the game data (dbc, maps, vmaps, mmaps) that the person already has. Used in place, read-only.
    pub data_dir: PathBuf,
    /// Public key to verify the manifest with (production: `signing::EMBEDDED_PUBLIC_KEY`).
    pub trusted_key: &'a str,
    pub registry: &'a Registry,
    pub cancel: Cancel,
}

/// Checks whether `dest` and the game data folder are sensible for a new server on Linux.
pub fn preflight(dest: &Path, data_dir: &Path, needed_bytes: u64, registry: &Registry) -> Preflight {
    let mut problems = Vec::new();
    let s = dest.to_string_lossy();

    if !dest.is_absolute() {
        problems.push(problem("relative", "Choose a full folder path, such as /home/you/CoaServer."));
    }
    if s.contains(':') {
        problems.push(problem("colon", "Choose a folder whose path has no colon in it."));
    }
    let trimmed = s.trim_end_matches('/');
    const SYSTEM: [&str; 13] = ["", "/bin", "/boot", "/dev", "/etc", "/lib", "/lib64", "/proc", "/run", "/sbin", "/sys", "/usr", "/var"];
    if SYSTEM.iter().any(|p| trimmed == *p || (!p.is_empty() && trimmed.starts_with(&format!("{p}/")))) {
        problems.push(problem("system_folder", "Please choose a normal folder for your games, not a system location."));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = home.to_string_lossy().trim_end_matches('/').to_string();
        let special = ["", "/Documents", "/Desktop", "/Downloads", "/Pictures", "/Music", "/Videos", "/.config", "/.local", "/.local/share"];
        if !home.is_empty() && special.iter().any(|sfx| trimmed == format!("{home}{sfx}")) {
            problems.push(problem("personal_folder", "This folder contains unrelated files. Choose another folder or create a new CoA Server folder."));
        }
    }

    if dest.exists() {
        match fs::read_dir(dest) {
            Ok(mut rd) => {
                if rd.next().is_some() {
                    match crate::layout::scan(dest).map(|r| r.classification) {
                        Ok(crate::layout::Classification::Incompatible) | Err(_) => {
                            if dest.join("Wow.exe").is_file() || dest.join("Ascension.exe").is_file() {
                                problems.push(problem("client_folder", "This is a game client folder, not a place for a server."));
                            } else {
                                problems.push(problem("not_empty", "This folder contains unrelated files. Choose another folder or create a new CoA Server folder."));
                            }
                        }
                        Ok(_) => problems.push(problem("already_server", "A server is already in this folder. Use \"I already have a server\" to add it instead.")),
                    }
                }
            }
            Err(_) => problems.push(problem("unreadable", "This folder cannot be read. Choose another one.")),
        }
    }

    if let Ok(list) = registry.list() {
        for (_, existing) in list {
            if let (Ok(a), Ok(b)) = (fsx::canonicalize_lenient(dest), fsx::canonicalize_lenient(&existing)) {
                if fsx::starts_with_ci(&a, &b) || fsx::starts_with_ci(&b, &a) {
                    problems.push(problem("registered", "A server is already registered at or around this location."));
                    break;
                }
            }
        }
    }

    problems.extend(data_problems(data_dir));

    let free_bytes = fsx::free_space(dest).unwrap_or(0);
    let need = needed_bytes.saturating_add(needed_bytes / 5).saturating_add(512 * 1024 * 1024);
    if free_bytes < need {
        problems.push(problem("space", &format!("Not enough free space: about {} GB needed, {} GB available.", need / (1 << 30) + 1, free_bytes / (1 << 30))));
    }
    Preflight { ok: problems.is_empty(), problems, free_bytes }
}

pub(crate) fn data_problems(data_dir: &Path) -> Vec<crate::install::Problem> {
    let s = data_dir.to_string_lossy();
    if !data_dir.is_absolute() || s.contains(':') {
        return vec![problem("data_path", "Choose the game data folder with its full path, without a colon in it.")];
    }
    let missing: Vec<&str> = ["dbc", "maps"].into_iter().filter(|d| !data_dir.join(d).is_dir()).collect();
    if missing.is_empty() {
        Vec::new()
    } else {
        vec![problem("data_missing", &format!("This folder does not look like the game data: {} not found in it. It should hold dbc, maps, vmaps and mmaps.", missing.join(" and ")))]
    }
}

/// Port the game should use: the usual one when it is free on this computer, else any free one.
fn pick_port(preferred: u16) -> Result<u16> {
    if std::net::TcpListener::bind(("127.0.0.1", preferred)).is_ok() {
        Ok(preferred)
    } else {
        free_port()
    }
}

/// Settings of a new installation: which containers belong to it, where the game data is, the ports, the passwords.
/// Written before anything runs, so the database can be started from the staging folder.
struct Settings {
    app_password: String,
    world_port: u16,
}

fn write_settings(root: &Path, data_dir: &Path) -> Result<Settings> {
    let project = random_hex(10);
    let (auth, world, ra) = (pick_port(DEFAULT_PORTS.0)?, pick_port(DEFAULT_PORTS.1)?, pick_port(DEFAULT_PORTS.2)?);
    let (root_password, app_password) = (random_hex(48), random_hex(48));
    fs::create_dir_all(root.join("Settings"))?;
    let docker = Config { project, bind_address: "127.0.0.1".into(), mysql_image: super::MYSQL_IMAGE.into(), data_dir: Some(data_dir.to_string_lossy().into_owned()), mysql_data: None };
    fsx::atomic_write_json(&root.join(MARKER), &docker)?;
    // Same file and keys as a repack, so the console, the ports and the settings screens work unchanged. The database is
    // not published on the host; its port is only there to fill the key.
    fsx::atomic_write_json(
        &root.join("Settings/repack.json"),
        &serde_json::json!({ "mysqlPort": 3307, "authPort": auth, "worldPort": world, "raPort": ra, "raUsername": db::SERVICE_ACCOUNT, "raPassword": "" }),
    )?;
    let secrets = root.join("Settings/database.json");
    fsx::atomic_write_json(&secrets, &serde_json::json!({ "rootPassword": root_password, "appPassword": app_password }))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&secrets, fs::Permissions::from_mode(0o600))?;
    }
    Ok(Settings { app_password, world_port: world })
}

/// The package must hold what a Docker installation is made of; say what is missing instead of failing later.
fn check_extracted(root: &Path) -> Result<()> {
    let mut missing = Vec::new();
    for rel in ["Core/worldserver", "Core/authserver", "Core/configs/worldserver.conf.dist", "Core/configs/authserver.conf.dist"] {
        if !root.join(rel).is_file() {
            missing.push(rel.to_string());
        }
    }
    for (kind, _) in DATABASES {
        if !root.join(BASELINE_DIR).join(format!("{kind}.sql.zst")).is_file() {
            missing.push(format!("{BASELINE_DIR}/{kind}.sql.zst"));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(format!("This package is not a complete Linux server package; missing: {}.", missing.join(", "))))
    }
}

/// Record every migration of the package as applied without running it, in a few statements (one `docker exec` for each
/// of more than a thousand would take minutes). Same ledger rows as `migrations::baseline`.
fn baseline_statements(list: &[Migration]) -> Result<Vec<String>> {
    let ok = |t: &str| !t.is_empty() && t.len() <= 190 && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    let mut rows = Vec::with_capacity(list.len());
    for m in list {
        if !matches!(m.db.as_str(), "auth" | "characters" | "world") || !ok(&m.id) || m.sha256.len() != 64 || !m.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidManifest(format!("migration {:?} is not valid", m.id)));
        }
        rows.push(format!("('{}','{}','{}','applied',NOW(),NULL,1)", m.db, m.id, m.sha256));
    }
    Ok(rows
        .chunks(400)
        .map(|c| format!("REPLACE INTO `acore_world`.`coa_manager_migrations` (`db`,`id`,`sha256`,`status`,`applied_at`,`error`,`baseline`) VALUES {};", c.join(",")))
        .collect())
}

/// Create the three schemas and the game servers' account, load the starting databases and record the package's migrations
/// as applied. The database container is started from `root` and stopped again by the caller.
fn build_database(d: &dyn Docker, root: &Path, list: &[Migration], s: &Settings, say: &dyn Fn(u8, String)) -> Result<()> {
    let started = super::run_with(d, root, Verb::StartMysql)?;
    if !started.ok {
        return Err(Error::Invalid(started.human.map(|h| h.message.to_string()).unwrap_or_else(|| "The database could not be started.".into())));
    }
    let db = Db::from_repack(root, Account::Admin)?;
    let mut sql = String::new();
    for (_, schema) in DATABASES {
        sql.push_str(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;\n"));
    }
    // The pattern also covers the schemas of other realms (acore_world_wildcard, ...).
    sql.push_str(&format!("CREATE USER 'acore'@'%' IDENTIFIED BY '{}';\nGRANT ALL PRIVILEGES ON `acore\\_%`.* TO 'acore'@'%';\nFLUSH PRIVILEGES;\n", s.app_password));
    db.query(&sql)?;

    for (i, (kind, schema)) in DATABASES.iter().enumerate() {
        say(83 + (i * 4) as u8, format!("Loading the {kind} database"));
        db.import_from(schema, &root.join(BASELINE_DIR).join(format!("{kind}.sql.zst")))?;
    }

    say(95 - 3, format!("Recording {} database updates as applied", list.len()));
    // The ledger table is created by the first read of it.
    migrations::Store::load(&db)?;
    for statement in baseline_statements(list)? {
        db.query(&statement)?;
    }

    // The realm advertises the port the game server really uses on this computer.
    db.query(&format!("UPDATE acore_auth.realmlist SET port={} WHERE id=1;", s.world_port))?;
    // The server console gets its own account with a random password.
    let console_pw = db.provision_service_account()?;
    db::write_console_credentials(root, &console_pw)?;
    if !Db::from_repack(root, Account::Admin)?.ping() {
        return Err(Error::Invalid("The database did not answer after it was set up.".into()));
    }
    Ok(())
}

pub fn install(p: &Params, report: &dyn Fn(Step)) -> Result<Installed> {
    install_with(&SystemDocker, p, report)
}

pub fn install_with(d: &dyn Docker, p: &Params, report: &dyn Fn(Step)) -> Result<Installed> {
    let say = |step: &'static str, percent: u8, detail: Option<String>| report(Step { step, percent, detail });
    let dest = p.dest.clone();
    let staging_root = {
        let mut n = dest.file_name().ok_or_else(|| Error::Invalid("bad destination".into()))?.to_os_string();
        n.push(".installing");
        dest.with_file_name(n)
    };
    let meta_dir = metadata_dir_for(&dest)?;

    say("Checking your computer", 2, None);
    super::check_docker(d)?;
    // The manifest first: it is signed, and tells us how much space we need.
    let (m, manifest_bytes) = fetch_manifest(&p.source, p.trusted_key)?;
    if !matches!(m.kind, manifest::Kind::Base | manifest::Kind::Update) || !m.compatible_with_manager(crate::MANAGER_VERSION) {
        return Err(Error::Invalid("This package needs a newer version of CoA Server Manager.".into()));
    }
    let archive = m.archive.clone().ok_or_else(|| Error::InvalidManifest("no archive".into()))?;
    let download_size: u64 = archive.parts.iter().map(|x| x.size).sum();

    let pre = preflight(&dest, &p.data_dir, archive.unpacked_size + download_size, p.registry);
    if !pre.ok {
        return Err(Error::Invalid(pre.problems.iter().map(|x| x.message.clone()).collect::<Vec<_>>().join(" ")));
    }

    let parts_dir = fetch_parts(&p.source, &m, &meta_dir.join("staging").join("download"), &p.cancel, &|frac, detail| say("Downloading server", 5 + (frac * 45.0) as u8, detail))?;

    if staging_root.exists() {
        if staging_root.join(STAGING_MARKER).is_file() {
            fs::remove_dir_all(&staging_root)?; // leftover of an earlier failed attempt that we created
        } else {
            return Err(Error::Invalid(format!("{} already exists and was not created by the Manager.", staging_root.display())));
        }
    }
    fs::create_dir_all(&staging_root)?;
    fs::write(staging_root.join(STAGING_MARKER), b"")?;

    let mut project: Option<Config> = None;
    let result = (|| -> Result<Installed> {
        say("Unpacking", 50, None);
        crate::package::extract(&parts_dir, &m, &staging_root, &|done, total| say("Unpacking", 50 + (done * 30 / total.max(1)) as u8, None))?;
        check_extracted(&staging_root)?;

        say("Preparing database", 82, None);
        let settings = write_settings(&staging_root, &p.data_dir)?;
        project = Some(Config::load(&staging_root)?);
        let progress = |percent: u8, detail: String| say("Preparing database", percent, Some(detail));
        let built = build_database(d, &staging_root, &m.migrations, &settings, &progress);
        let _ = super::run_with(d, &staging_root, Verb::StopAll);
        built?;
        fs::remove_file(staging_root.join(STAGING_MARKER))?;

        // Commit: the only moment the destination changes. The database lives in a docker volume, so it does not care
        // that the folder moves.
        say("Finishing", 95, None);
        if dest.exists() {
            fs::remove_dir(&dest)?; // only succeeds for an empty folder (preflight verified)
        }
        fs::rename(&staging_root, &dest)?;
        super::ensure_main_configs(&dest)?;
        crate::config::materialize_module_configs(&dest)?;

        let mut meta = InstallMeta::new(InstallKind::New, &dest);
        meta.layout = LAYOUT_DOCKER_V1.into();
        meta.core.commit = m.core.commit.clone();
        meta.core.version = Some(m.version.clone());
        meta.database.schemas = DATABASES.iter().map(|(_, s)| s.to_string()).collect();
        meta.managed_files = m.files.iter().map(|f| f.path.clone()).collect();
        for f in &m.files {
            meta.original_hashes.insert(f.path.clone(), f.sha256.clone());
        }
        let md = MetaDir::create(&dest, &meta)?;
        fsx::atomic_write(&md.root.join("manifests").join("base.json"), &manifest_bytes)?;
        p.registry.register(&meta.id, &dest)?;
        say("Ready", 100, None);
        Ok(Installed { id: meta.id, path: dest.to_string_lossy().into_owned(), version: m.version.clone() })
    })();

    if result.is_err() && staging_root.join(STAGING_MARKER).exists() {
        // Unfinished work of ours; the destination was never touched. Give back what docker holds for it.
        if let Some(cfg) = &project {
            super::destroy(d, cfg);
        }
        let _ = fs::remove_dir_all(&staging_root);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::cli::{Call, Output};
    use crate::package::{self, BuildOptions};
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<Vec<String>>>,
        unavailable: bool,
    }

    impl Fake {
        fn new(unavailable: bool) -> Fake {
            Fake { calls: Default::default(), unavailable }
        }
    }

    impl Docker for Fake {
        fn run(&self, call: &Call) -> Result<Output> {
            self.calls.borrow_mut().push(call.args.clone());
            if self.unavailable {
                return Err(Error::Invalid("docker could not be started: not found".into()));
            }
            Ok(Output { code: Some(0), stdout: "27.3.1".into(), stderr: String::new() })
        }

        fn pause(&self, _d: std::time::Duration) {}
    }

    fn reg(d: &Path) -> Registry {
        Registry::at(d.join("reg/installs.json"))
    }

    fn codes(p: &Preflight) -> Vec<&'static str> {
        p.problems.iter().map(|x| x.code).collect()
    }

    fn data_folder(d: &Path) -> PathBuf {
        let data = d.join("data");
        fs::create_dir_all(data.join("dbc")).unwrap();
        fs::create_dir_all(data.join("maps")).unwrap();
        data
    }

    /// A signed package folder holding `files`, and the key that verifies it.
    fn signed_package(d: &Path, files: &[(&str, &str)]) -> (PathBuf, String) {
        let src = d.join("src");
        for (rel, content) in files {
            let p = src.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        let pkg = d.join("pkg");
        let opts = BuildOptions { kind: manifest::Kind::Update, version: "0.1.0".into(), core_commit: None, built_at: "x".into(), part_size: 1 << 20, bots_commit: None, migrations: vec![] };
        package::build(&src, &pkg, &opts, &|_| {}).unwrap();
        let key = SigningKey::generate(&mut rand_core::OsRng);
        let engine = base64::engine::general_purpose::STANDARD;
        let sig = engine.encode(key.sign(&fs::read(pkg.join("manifest.json")).unwrap()).to_bytes());
        fs::write(pkg.join("manifest.json.sig"), sig).unwrap();
        (pkg, engine.encode(key.verifying_key().to_bytes()))
    }

    const FULL: [(&str, &str); 7] = [
        ("Core/worldserver", "w"),
        ("Core/authserver", "a"),
        ("Core/configs/worldserver.conf.dist", "x"),
        ("Core/configs/authserver.conf.dist", "x"),
        ("Database/baseline/auth.sql.zst", "dump"),
        ("Database/baseline/characters.sql.zst", "dump"),
        ("Database/baseline/world.sql.zst", "dump"),
    ];

    #[cfg(unix)]
    #[test]
    fn the_destination_rules_fit_a_linux_computer() {
        let d = tempfile::tempdir().unwrap();
        let data = data_folder(d.path());
        let ok = |dest: &Path| codes(&preflight(dest, &data, 1000, &reg(d.path())));
        assert!(!ok(&d.path().join("CoaServer")).iter().any(|c| *c != "space"), "a new folder is fine");
        for bad in ["/usr/local/coa", "/etc", "/", "/proc/x"] {
            assert!(ok(Path::new(bad)).contains(&"system_folder"), "{bad}");
        }
        assert!(ok(Path::new("relative/dir")).contains(&"relative"));
        assert!(ok(Path::new("/srv/with:colon")).contains(&"colon"));
        let existing = d.path().join("existing");
        fs::create_dir_all(&existing).unwrap();
        fs::write(existing.join("notes.txt"), b"x").unwrap();
        assert!(ok(&existing).contains(&"not_empty"));
        let r = reg(d.path());
        r.register("1", &existing).unwrap();
        assert!(codes(&preflight(&existing.join("inner"), &data, 1000, &r)).contains(&"registered"));
        // The same name in another case is another folder on Linux.
        assert!(!codes(&preflight(&d.path().join("EXISTING"), &data, 1000, &r)).contains(&"registered"));
    }

    // The tests marked for unix use absolute folders of the temporary directory, which on Windows start with a drive letter and a
    // colon; the Docker backend refuses a colon in a folder it mounts (it would be read as a separator of `--volume`) and only runs
    // on Linux, so Windows has nothing to check here.
    #[cfg(unix)]
    #[test]
    fn the_game_data_folder_must_look_like_game_data() {
        let d = tempfile::tempdir().unwrap();
        let reg = reg(d.path());
        let dest = d.path().join("srv");
        let empty = d.path().join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(codes(&preflight(&dest, &empty, 1, &reg)).contains(&"data_missing"));
        assert!(codes(&preflight(&dest, Path::new("relative/data"), 1, &reg)).contains(&"data_path"));
        assert!(!codes(&preflight(&dest, &data_folder(d.path()), 1, &reg)).iter().any(|c| c.starts_with("data")));
    }

    #[cfg(unix)]
    #[test]
    fn new_settings_name_the_containers_and_keep_the_secrets_private() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        let data = data_folder(d.path());
        let s = write_settings(&root, &data).unwrap();

        let cfg = Config::load(&root).unwrap();
        assert_eq!(cfg.bind_address, "127.0.0.1", "this computer only until the person opens it");
        assert_eq!(cfg.project.len(), 10);
        assert_eq!(cfg.data_path(&root), data);

        let ports = crate::layout::read_ports(&root);
        assert_eq!(ports.world, s.world_port);
        assert!(ports.auth > 0 && ports.ra > 0);
        let repack: serde_json::Value = fsx::read_json(&root.join("Settings/repack.json")).unwrap();
        assert_eq!(repack["raUsername"], db::SERVICE_ACCOUNT);

        let (root_pw, app_pw) = db::credentials(&root).unwrap();
        assert!(root_pw.len() == 48 && app_pw.len() == 48 && root_pw != app_pw);
        assert_eq!(app_pw, s.app_password);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(root.join("Settings/database.json")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the passwords are for their owner only");
        }
        // Two installations never share a project name.
        let other = write_settings(&d.path().join("srv2"), &data).unwrap();
        assert_ne!(Config::load(&d.path().join("srv2")).unwrap().project, cfg.project);
        let _ = other;
    }

    #[test]
    fn a_busy_default_port_is_replaced_by_a_free_one() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = held.local_addr().unwrap().port();
        let got = pick_port(busy).unwrap();
        assert_ne!(got, busy);
        drop(held);
        assert_eq!(pick_port(busy).unwrap(), busy, "free again: the usual port is used");
    }

    #[test]
    fn a_package_is_checked_for_everything_an_installation_needs() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        assert!(check_extracted(root).unwrap_err().to_string().contains("Core/worldserver"));
        for (rel, c) in FULL {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        }
        check_extracted(root).unwrap();
        fs::remove_file(root.join("Database/baseline/world.sql.zst")).unwrap();
        assert!(check_extracted(root).unwrap_err().to_string().contains("Database/baseline/world.sql.zst"));
    }

    fn migration(db: &str, id: &str) -> Migration {
        Migration { db: db.into(), id: id.into(), sha256: "ab".repeat(32), destructive: false, compatible_sha256: vec![] }
    }

    #[test]
    fn the_package_migrations_are_recorded_as_applied_in_a_few_statements() {
        let list: Vec<Migration> = (0..1000).map(|i| migration(["auth", "characters", "world"][i % 3], &format!("rev_{i}"))).collect();
        let statements = baseline_statements(&list).unwrap();
        assert_eq!(statements.len(), 3, "400 rows at most in a statement");
        assert!(statements[0].starts_with("REPLACE INTO `acore_world`.`coa_manager_migrations`"));
        assert_eq!(statements.iter().map(|s| s.matches("'applied'").count()).sum::<usize>(), 1000);
        assert!(statements[0].contains("('auth','rev_0',") && statements[0].contains(",NOW(),NULL,1)"), "marked as a baseline");
        assert!(baseline_statements(&[]).unwrap().is_empty());
    }

    #[test]
    fn a_migration_that_could_carry_sql_is_refused_before_anything_is_written() {
        for bad in [migration("world", "x'); DROP TABLE y;--"), migration("mysql", "ok"), migration("world", ""), Migration { sha256: "zz".into(), ..migration("world", "ok") }] {
            assert!(baseline_statements(&[migration("auth", "fine"), bad]).is_err());
        }
    }

    #[test]
    fn without_docker_nothing_is_downloaded_or_created() {
        let d = tempfile::tempdir().unwrap();
        let (pkg, key) = signed_package(d.path(), &FULL);
        let dest = d.path().join("dest");
        let r = reg(d.path());
        let fake = Fake::new(true);
        let err = install_with(&fake, &Params { source: Source::Dir(pkg), dest: dest.clone(), data_dir: data_folder(d.path()), trusted_key: &key, registry: &r, cancel: Cancel::default() }, &|_| {}).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("docker"), "{err}");
        assert!(!dest.exists() && !dest.with_file_name("dest.installing").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_package_without_the_server_is_refused_and_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let (pkg, key) = signed_package(d.path(), &[("Core/configs/worldserver.conf.dist", "x"), ("README.txt", "hello")]);
        let dest = d.path().join("dest");
        let r = reg(d.path());
        let fake = Fake::new(false);
        let err = install_with(&fake, &Params { source: Source::Dir(pkg), dest: dest.clone(), data_dir: data_folder(d.path()), trusted_key: &key, registry: &r, cancel: Cancel::default() }, &|_| {}).unwrap_err();
        assert!(err.to_string().contains("not a complete Linux server package"), "{err}");
        assert!(!dest.exists() && !dest.with_file_name("dest.installing").exists());
        assert!(r.list().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_database_setup_gives_back_what_docker_holds_and_leaves_no_server() {
        let d = tempfile::tempdir().unwrap();
        let (pkg, key) = signed_package(d.path(), &FULL);
        let dest = d.path().join("dest");
        let r = reg(d.path());
        // This fake docker answers every command with success and no container, so the database never becomes ready.
        let fake = Fake::new(false);
        let err = install_with(&fake, &Params { source: Source::Dir(pkg), dest: dest.clone(), data_dir: data_folder(d.path()), trusted_key: &key, registry: &r, cancel: Cancel::default() }, &|_| {}).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("database"), "{err}");
        assert!(!dest.exists() && !dest.with_file_name("dest.installing").exists(), "no half-installed server");
        assert!(r.list().unwrap().is_empty());
        let calls = fake.calls.borrow();
        let removed = |suffix: &str| calls.iter().any(|c| c[0] == "rm" && c.last().is_some_and(|n| n.starts_with("coa-") && n.ends_with(suffix)));
        assert!(removed("-db") && removed("-world") && removed("-auth"), "containers removed: {calls:?}");
        assert!(calls.iter().any(|c| c[..2] == ["volume", "rm"]) && calls.iter().any(|c| c[..2] == ["network", "rm"]));
    }
}
