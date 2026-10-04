//! Clean installation of a base package.
//!
//! Order matters: nothing appears at the destination until the package is signed-off, downloaded, verified,
//! extracted and its database credentials rotated. Everything before the final rename happens in a
//! sibling `<dest>.installing` folder that this module created and marks, so a failure or crash can only ever
//! leave that folder (safe to discard) - never a half-installed server at the destination.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::db::{self, Account, Db};
use crate::download::Cancel;
use crate::driver::{self, Verb};
use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::{self, Classification};
use crate::manifest;
use crate::package::{self, BOOTSTRAP_CREDENTIALS};
use crate::registry::{metadata_dir_for, InstallKind, InstallMeta, MetaDir, Registry};

const MARKER: &str = ".coa-installing";

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct Preflight {
    pub ok: bool,
    pub problems: Vec<Problem>,
    pub free_bytes: u64,
}

fn problem(code: &'static str, message: &str) -> Problem {
    Problem { code, message: message.into() }
}

/// Checks whether `dest` is a sensible place for a new server (spec section 32).
pub fn preflight(dest: &Path, needed_bytes: u64, registry: &Registry) -> Preflight {
    let mut problems = Vec::new();
    let s = dest.to_string_lossy();

    if !dest.is_absolute() {
        problems.push(problem("relative", "Choose a full folder path, such as C:\\Games\\CoA Server."));
    }
    if !s.is_ascii() {
        problems.push(problem("non_ascii", "Choose a folder whose path uses only English letters and digits, such as C:\\Games\\CoA Server."));
    }
    if s.len() > 100 {
        problems.push(problem("too_long", "That folder path is too long. Choose a shorter one."));
    }
    let lower = s.to_lowercase().replace('/', "\\");
    let trimmed = lower.trim_end_matches('\\');
    let protected_roots = ["c:\\windows", "c:\\program files", "c:\\program files (x86)", "c:\\programdata"];
    if trimmed.len() <= 3 || protected_roots.iter().any(|p| trimmed == *p || trimmed.starts_with(&format!("{p}\\"))) {
        problems.push(problem("system_folder", "Please choose a normal folder for your games, not a system location or a drive root."));
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let profile = profile.to_string_lossy().to_lowercase();
        let special = ["", "\\documents", "\\desktop", "\\downloads", "\\pictures", "\\music", "\\videos"];
        if special.iter().any(|sfx| trimmed == format!("{profile}{sfx}")) {
            problems.push(problem("personal_folder", "This folder contains unrelated files. Choose another folder or create a new CoA Server folder."));
        }
    }

    if dest.exists() {
        match fs::read_dir(dest) {
            Ok(mut rd) => {
                if rd.next().is_some() {
                    match layout::scan(dest).map(|r| r.classification) {
                        Ok(Classification::Incompatible) | Err(_) => {
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
            let (a, b) = (fsx::canonicalize_lenient(dest), fsx::canonicalize_lenient(&existing));
            if let (Ok(a), Ok(b)) = (a, b) {
                if fsx::starts_with_ci(&a, &b) || fsx::starts_with_ci(&b, &a) {
                    problems.push(problem("registered", "A server is already registered at or around this location."));
                    break;
                }
            }
        }
    }

    let free_bytes = fsx::free_space(dest).unwrap_or(0);
    let need = needed_bytes.saturating_add(needed_bytes / 5).saturating_add(512 * 1024 * 1024);
    if free_bytes < need {
        problems.push(problem("space", &format!("Not enough free space: about {} GB needed, {} GB available.", need / (1 << 30) + 1, free_bytes / (1 << 30))));
    }
    Preflight { ok: problems.is_empty(), problems, free_bytes }
}

pub use crate::pkgsource::Source;
use crate::pkgsource::{fetch_manifest, fetch_parts};

#[derive(Debug, Clone, Serialize)]
pub struct Step {
    pub step: &'static str,
    pub percent: u8,
    pub detail: Option<String>,
}

pub struct Params<'a> {
    pub source: Source,
    pub dest: PathBuf,
    /// Public key to verify the manifest with (production: `signing::EMBEDDED_PUBLIC_KEY`).
    pub trusted_key: &'a str,
    pub registry: &'a Registry,
    pub cancel: Cancel,
}

#[derive(Debug, Serialize)]
pub struct Installed {
    pub id: String,
    pub path: String,
    pub version: String,
}

fn random_hex(n: usize) -> String {
    let mut s = String::new();
    while s.len() < n {
        s.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    s.truncate(n);
    s
}

/// Rotate the database users' passwords away from the packaged bootstrap values and write the launcher's
/// `Settings/database.json`. MySQL is started from `root` and stopped again before returning.
pub(crate) fn bootstrap_database(root: &Path) -> Result<()> {
    with_scratch_ports(root, || bootstrap_database_inner(root))
}

fn free_port() -> Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// Run `f` with the staging copy listening on unused ports, so an installation never collides with a server (or any
/// other program) already using the shipped ports. The shipped ports are put back afterwards.
pub(crate) fn with_scratch_ports<T>(root: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    const KEYS: [&str; 4] = ["mysqlPort", "authPort", "worldPort", "raPort"];
    let path = root.join("Settings/repack.json");
    let original: serde_json::Value = fsx::read_json(&path)?;
    let mut scratch = original.clone();
    for k in KEYS {
        scratch[k] = free_port()?.into();
    }
    fsx::atomic_write_json(&path, &scratch)?;
    let result = f();
    // keep whatever else changed meanwhile (console credentials), restore only the ports
    let mut now: serde_json::Value = fsx::read_json(&path).unwrap_or(scratch);
    for k in KEYS {
        now[k] = original[k].clone();
    }
    fsx::atomic_write_json(&path, &now)?;
    result
}

fn bootstrap_database_inner(root: &Path) -> Result<()> {
    let boot = root.join(BOOTSTRAP_CREDENTIALS);
    let target = root.join("Settings/database.json");
    if !boot.is_file() {
        return Err(Error::Invalid("The package has no database bootstrap information.".into()));
    }
    fs::copy(&boot, &target)?; // launcher needs database.json to render its files at first start

    let started = driver::run(root, Verb::StartMysql)?;
    if !started.ok {
        return Err(Error::Invalid(started.human.map(|h| h.message.to_string()).unwrap_or_else(|| "The database could not be started.".into())));
    }
    let result = (|| -> Result<()> {
        let db = Db::from_repack(root, Account::Admin)?;
        let (root_pw, app_pw) = (random_hex(48), random_hex(48));
        let accounts = db.query("SELECT CONCAT(user,'@',host) FROM mysql.user WHERE user IN ('root','acore');")?;
        let mut sql = String::new();
        for line in accounts.lines() {
            let (user, host) = line.split_once('@').ok_or_else(|| Error::Invalid("unexpected user list".into()))?;
            let pw = if user == "root" { &root_pw } else { &app_pw };
            if !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | ':' | '-')) {
                return Err(Error::Invalid("unexpected database host name".into()));
            }
            sql.push_str(&format!("ALTER USER '{user}'@'{host}' IDENTIFIED BY '{pw}';\n"));
        }
        sql.push_str("FLUSH PRIVILEGES;\n");
        db.query(&sql)?;
        fsx::atomic_write_json(&target, &serde_json::json!({ "rootPassword": root_pw, "appPassword": app_pw }))?;
        // The server console gets its own account with a random password instead of a shared default.
        let console_pw = Db::from_repack(root, Account::Admin)?.provision_service_account()?; // new root password
        db::write_console_credentials(root, &console_pw)?;
        // The new root password must actually work before we throw the old one away.
        if !Db::from_repack(root, Account::Admin)?.ping() {
            return Err(Error::Invalid("The new database password did not work.".into()));
        }
        Ok(())
    })();
    let _ = driver::run(root, Verb::StopAll);
    result?;
    fs::remove_file(&boot)?;
    Ok(())
}

pub fn install_base(p: &Params, report: &dyn Fn(Step)) -> Result<Installed> {
    let say = |step: &'static str, percent: u8, detail: Option<String>| report(Step { step, percent, detail });
    let dest = p.dest.clone();
    let staging_root = {
        let mut n = dest.file_name().ok_or_else(|| Error::Invalid("bad destination".into()))?.to_os_string();
        n.push(".installing");
        dest.with_file_name(n)
    };
    let meta_dir = metadata_dir_for(&dest)?;

    say("Checking your computer", 2, None);
    // 1. Manifest first: it is signed, and tells us how much space we need.
    let (m, manifest_bytes) = fetch_manifest(&p.source, p.trusted_key)?;
    if m.kind != manifest::Kind::Base || !m.compatible_with_manager(crate::MANAGER_VERSION) {
        return Err(Error::Invalid("This package needs a newer version of CoA Server Manager.".into()));
    }
    let archive = m.archive.clone().ok_or_else(|| Error::InvalidManifest("no archive".into()))?;
    let download_size: u64 = archive.parts.iter().map(|x| x.size).sum();

    let pre = preflight(&dest, archive.unpacked_size + download_size, p.registry);
    if !pre.ok {
        return Err(Error::Invalid(pre.problems.iter().map(|x| x.message.clone()).collect::<Vec<_>>().join(" ")));
    }

    // 2. Download (or use the local folder), verifying every part.
    let parts_dir = fetch_parts(&p.source, &m, &meta_dir.join("staging").join("download"), &p.cancel, &|frac, detail| {
        say("Downloading server", 5 + (frac * 45.0) as u8, detail)
    })?;

    // 3. Extract into our own staging folder.
    if staging_root.exists() {
        if staging_root.join(MARKER).is_file() {
            fs::remove_dir_all(&staging_root)?; // leftover of an earlier failed attempt that we created
        } else {
            return Err(Error::Invalid(format!("{} already exists and was not created by the Manager.", staging_root.display())));
        }
    }
    fs::create_dir_all(&staging_root)?;
    fs::write(staging_root.join(MARKER), b"")?;

    let result = (|| -> Result<Installed> {
        say("Unpacking", 50, None);
        package::extract(&parts_dir, &m, &staging_root, &|done, total| say("Unpacking", 50 + (done * 30 / total.max(1)) as u8, None))?;
        say("Preparing database", 82, None);
        bootstrap_database(&staging_root)?;
        crate::backup::with_database(&staging_root, |db| {
            let problems = crate::schema_check::check(db, &staging_root)?;
            if let Some(p) = problems.first() {
                return Err(Error::Invalid(format!("The packaged database is incomplete: {}.{}.{}: {}.", p.database, p.table, p.column, p.detail)));
            }
            Ok(())
        })?;
        fs::remove_file(staging_root.join(MARKER))?;

        // 4. Commit: the only moment the destination changes.
        say("Finishing", 95, None);
        if dest.exists() {
            fs::remove_dir(&dest)?; // only succeeds for an empty folder (preflight verified)
        }
        fs::rename(&staging_root, &dest)?;
        crate::config::materialize_module_configs(&dest)?;

        let mut meta = InstallMeta::new(InstallKind::New, &dest);
        meta.core.commit = m.core.commit.clone();
        meta.core.version = Some(m.version.clone());
        meta.database.schemas = vec!["acore_auth".into(), "acore_characters".into(), "acore_world".into()];
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

    if result.is_err() && staging_root.join(MARKER).exists() {
        // Unfinished work of ours; the destination was never touched.
        let _ = driver::run(&staging_root, Verb::StopAll);
        let _ = fs::remove_dir_all(&staging_root);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(d: &Path) -> Registry {
        Registry::at(d.join("reg/installs.json"))
    }

    fn codes(p: &Preflight) -> Vec<&'static str> {
        p.problems.iter().map(|x| x.code).collect()
    }

    #[test]
    fn accepts_a_new_or_empty_folder() {
        let d = tempfile::tempdir().unwrap();
        // temp dirs may contain non-ASCII user names; only test the rules that don't depend on that
        let fresh = d.path().join("CoA Server");
        let p = preflight(&fresh, 1000, &reg(d.path()));
        assert!(!codes(&p).contains(&"not_empty") && !codes(&p).contains(&"already_server"));
        fs::create_dir_all(&fresh).unwrap();
        let p = preflight(&fresh, 1000, &reg(d.path()));
        assert!(!codes(&p).contains(&"not_empty"), "an empty folder is fine");
    }

    #[test]
    fn refuses_unrelated_client_and_existing_server_folders() {
        let d = tempfile::tempdir().unwrap();
        let docs = d.path().join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("cv.docx"), b"x").unwrap();
        assert!(codes(&preflight(&docs, 1, &reg(d.path()))).contains(&"not_empty"));

        let client = d.path().join("wow");
        fs::create_dir_all(client.join("Data")).unwrap();
        fs::write(client.join("Wow.exe"), b"x").unwrap();
        assert!(codes(&preflight(&client, 1, &reg(d.path()))).contains(&"client_folder"));

        let server = d.path().join("server");
        layout::testkit::fake_repack(&server);
        assert!(codes(&preflight(&server, 1, &reg(d.path()))).contains(&"already_server"));
    }

    #[test]
    fn refuses_system_folders_relative_paths_and_registered_overlaps() {
        let d = tempfile::tempdir().unwrap();
        for bad in ["C:\\", "C:\\Windows\\coa", "C:\\Program Files\\coa"] {
            assert!(codes(&preflight(Path::new(bad), 1, &reg(d.path()))).contains(&"system_folder"), "{bad}");
        }
        assert!(codes(&preflight(Path::new("relative\\dir"), 1, &reg(d.path()))).contains(&"relative"));
        let r = reg(d.path());
        let existing = d.path().join("existing");
        fs::create_dir_all(&existing).unwrap();
        r.register("1", &existing).unwrap();
        assert!(codes(&preflight(&existing.join("inner"), 1, &r)).contains(&"registered"));
    }

    #[test]
    fn refuses_when_there_is_not_enough_space() {
        let d = tempfile::tempdir().unwrap();
        assert!(codes(&preflight(&d.path().join("x"), u64::MAX / 4, &reg(d.path()))).contains(&"space"));
    }

    #[test]
    fn non_ascii_paths_are_rejected_because_the_launcher_cannot_run_from_them() {
        let d = tempfile::tempdir().unwrap();
        assert!(codes(&preflight(Path::new("C:\\Игры\\CoA"), 1, &reg(d.path()))).contains(&"non_ascii"));
    }

    #[test]
    fn random_credentials_have_the_launcher_required_shape() {
        let a = random_hex(48);
        assert_eq!(a.len(), 48);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_ne!(a, random_hex(48));
    }

    #[test]
    fn an_unsigned_or_wrongly_signed_package_is_refused_before_any_download() {
        use base64::Engine;
        use ed25519_dalek::{Signer, SigningKey};
        let d = tempfile::tempdir().unwrap();
        let pkg = d.path().join("pkg");
        let src = d.path().join("src");
        layout::testkit::fake_repack(&src);
        fs::create_dir_all(src.join("Settings")).unwrap();
        fs::write(src.join("Settings/database.json"), br#"{"rootPassword":"a","appPassword":"b"}"#).unwrap();
        let opts = package::BuildOptions { kind: manifest::Kind::Base, version: "0.1.0".into(), core_commit: None, built_at: "x".into(), part_size: 1 << 20, bots_commit: None, migrations: vec![] };
        package::build(&src, &pkg, &opts, &|_| {}).unwrap();
        let good = SigningKey::generate(&mut rand_core::OsRng);
        let evil = SigningKey::generate(&mut rand_core::OsRng);
        let mbytes = fs::read(pkg.join("manifest.json")).unwrap();
        let enc = |k: &SigningKey| base64::engine::general_purpose::STANDARD.encode(k.sign(&mbytes).to_bytes());
        let trusted = base64::engine::general_purpose::STANDARD.encode(good.verifying_key().to_bytes());
        let r = reg(d.path());
        let dest = d.path().join("dest");
        let run = |sig: &str| {
            fs::write(pkg.join("manifest.json.sig"), sig).unwrap();
            install_base(&Params { source: Source::Dir(pkg.clone()), dest: dest.clone(), trusted_key: &trusted, registry: &r, cancel: Cancel::default() }, &|_| {})
        };
        assert!(run(&enc(&evil)).is_err(), "signed by the wrong key");
        assert!(run("").is_err(), "empty signature");
        assert!(!dest.exists() && !dest.with_file_name("dest.installing").exists(), "nothing created when the signature is bad");
    }
}
