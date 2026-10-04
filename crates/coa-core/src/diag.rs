//! Diagnostics: a fixed list of read-only checks with plain-language results, a verification of the Manager's own files,
//! and a redacted diagnostic package for bug reports. Nothing here changes the server.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::parser::ConfFile;
use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::{self, Classification};
use crate::process::{self, ServiceState};
use crate::registry::InstallMeta;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub id: &'static str,
    pub title: &'static str,
    pub level: Level,
    pub detail: String,
}

fn check(id: &'static str, title: &'static str, level: Level, detail: impl Into<String>) -> Check {
    Check { id, title, level, detail: detail.into() }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub checks: Vec<Check>,
    pub problems: usize,
}

const MIN_FREE_BYTES: u64 = 5 * 1024 * 1024 * 1024;

pub fn run(root: &Path, meta: &InstallMeta) -> Report {
    let mut c = Vec::new();
    let scan = layout::scan(root);
    match &scan {
        Ok(r) => {
            let (lvl, text) = match r.classification {
                Classification::Healthy => (Level::Ok, "All expected server parts were found."),
                Classification::Partial => (Level::Warn, "Some server parts are missing."),
                Classification::UnknownCustom => (Level::Warn, "This is a custom server build; some features are limited."),
                Classification::Incompatible => (Level::Fail, "This folder does not look like a CoA server."),
            };
            c.push(check("files", "Server files", lvl, text));
            let missing: Vec<&str> = r.items.iter().filter(|i| i.status == layout::Status::Missing && i.key != "companions" && i.key != "release_info").map(|i| i.label).collect();
            if !missing.is_empty() {
                c.push(check("missing_parts", "Missing parts", Level::Warn, missing.join(", ")));
            }
        }
        Err(e) => c.push(check("files", "Server files", Level::Fail, e.to_string())),
    }

    let ports = layout::read_ports(root);
    let o = process::observe(root, &ports);
    let mut services = vec![("Database", &o.mysql), ("Login server", &o.auth), ("World server", &o.world)];
    if let Some(second) = &o.secondary_world { services.push(("Second world", second)); }
    for (title, s) in services {
        let (lvl, text) = match (s.state, &s.conflict) {
            (ServiceState::Running, _) => (Level::Ok, "Running".to_string()),
            (_, Some(conf)) => (Level::Fail, format!("Port {} is used by another program (process {}).", conf.port, conf.pid)),
            (ServiceState::Starting, _) => (Level::Warn, "Starting, not answering yet.".to_string()),
            _ => (Level::Warn, "Not running.".to_string()),
        };
        c.push(check(s.name, title, lvl, text));
    }

    // configuration files must at least be readable text the Manager can edit safely
    let mut unreadable = Vec::new();
    for rel in ["Core/configs/worldserver.conf", "Core/configs/authserver.conf", "Settings/worldserver.conf.template", "Settings/authserver.conf.template"] {
        let p = root.join(rel);
        if p.is_file() && fs::read(&p).map(|b| ConfFile::parse_bytes(&b).is_err()).unwrap_or(true) {
            unreadable.push(rel);
        }
    }
    c.push(if unreadable.is_empty() { check("configs", "Configuration", Level::Ok, "Configuration files can be read.") } else { check("configs", "Configuration", Level::Fail, format!("Cannot read: {}", unreadable.join(", "))) });
    for scope in [crate::config::Scope::Server, crate::config::Scope::Bots] {
        if let Ok(v) = crate::config::load(root, scope) {
            let bad: Vec<String> = v.settings.iter().filter(|s| s.problem.is_some()).map(|s| s.meta.key.clone()).collect();
            if !bad.is_empty() {
                c.push(check("config_values", "Setting values", Level::Warn, format!("Unusable values for: {}", bad.join(", "))));
            }
        }
    }

    match fsx::free_space(root) {
        Ok(f) if f >= MIN_FREE_BYTES => c.push(check("disk", "Free disk space", Level::Ok, format!("{} GB free", f >> 30))),
        Ok(f) => c.push(check("disk", "Free disk space", Level::Warn, format!("Only {} GB free; backups and updates need room.", f >> 30))),
        Err(e) => c.push(check("disk", "Free disk space", Level::Warn, e.to_string())),
    }

    let probe = root.join(".coa-write-test");
    let writable = fs::write(&probe, b"x").is_ok();
    let _ = fs::remove_file(&probe);
    c.push(if writable { check("permissions", "Permissions", Level::Ok, "The server folder is writable.") } else { check("permissions", "Permissions", Level::Fail, "The Manager cannot write to the server folder (try another location or run once as administrator).") });

    if let Some(path) = &meta.client_path {
        c.push(if crate::client::detect(Path::new(path), None).is_some() { check("client", "Game client", Level::Ok, path.clone()) } else { check("client", "Game client", Level::Warn, "The saved game folder was not found; choose it again in Settings.") });
    }

    let exposed: Vec<&str> = crate::net::exposure(&ports).iter().filter(|e| (e.what == "database" || e.what == "server console") && e.reachable_from_network).map(|e| e.what).collect();
    c.push(if exposed.is_empty() { check("exposure", "Private services", Level::Ok, "Database and server console are not reachable from the network.") } else { check("exposure", "Private services", Level::Fail, format!("Reachable from the network: {}.", exposed.join(", "))) });

    let problems = c.iter().filter(|x| x.level != Level::Ok).count();
    Report { checks: c, problems }
}

#[derive(Debug, Serialize)]
pub struct FileProblem {
    pub path: String,
    pub kind: &'static str,
}

/// Compare managed files with the hashes the Manager recorded. Read-only; user files are never listed.
pub fn verify_managed(root: &Path, meta: &InstallMeta) -> Vec<FileProblem> {
    let mut out = Vec::new();
    for (rel, want) in &meta.original_hashes {
        let lower = rel.replace('\\', "/").to_lowercase();
        if lower.starts_with("mysql/data/") || lower.starts_with("settings/") || lower.starts_with("core/configs/") && !lower.ends_with(".dist") { continue; }
        let Ok(p) = fsx::safe_join(root, rel) else { continue };
        match fsx::sha256_file(&p) {
            Err(_) => out.push(FileProblem { path: rel.clone(), kind: "missing" }),
            Ok(h) if !h.eq_ignore_ascii_case(want) => out.push(FileProblem { path: rel.clone(), kind: "changed" }),
            Ok(_) => {}
        }
    }
    out
}

fn tail(path: &Path, max: u64) -> Vec<u8> {
    let Ok(mut f) = fs::File::open(path) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
    let mut b = Vec::new();
    let _ = f.take(max).read_to_end(&mut b);
    b
}

/// Keep the first line of each repeated "Missing property X" warning and report how often it came.
pub fn squash_repeated_config_warnings(text: &str) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        let key = line.strip_prefix("> Config: Missing property ").and_then(|r| r.split_whitespace().next());
        match key {
            Some(k) => match counts.iter_mut().find(|(n, _)| n == k) {
                Some((_, c)) => *c += 1,
                None => {
                    counts.push((k.to_string(), 1));
                    out.push(line);
                }
            },
            None => out.push(line),
        }
    }
    let mut s = out.join("\n");
    for (k, c) in counts.iter().filter(|(_, c)| *c > 1) {
        s.push_str(&format!("
[Manager: \"Missing property {k}\" was logged {c} times in this excerpt]"));
    }
    s
}

/// Remove things that must never leave the machine from log text.
pub fn redact(text: &str) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.to_lowercase();
        let sensitive = ["password", "passwd", "secret", "token", "apikey", "api_key", "databaseinfo"].iter().any(|k| l.contains(k));
        out.push(if sensitive { "[line removed: may contain a secret]".to_string() } else { line.to_string() });
    }
    out.join("\n")
}

/// Zip with what a maintainer needs to debug a problem, with secrets removed. Returns the number of files inside.
pub fn export_package(root: &Path, meta_dir: &Path, manager_log: &Path, meta: &InstallMeta, report: &Report, out_zip: &Path) -> Result<usize> {
    let file = fs::File::create(out_zip)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut n = 0;
    let mut add = |name: &str, bytes: &[u8]| -> Result<()> {
        zip.start_file(name, opts).map_err(|e| Error::Invalid(e.to_string()))?;
        zip.write_all(bytes)?;
        n += 1;
        Ok(())
    };
    let summary = serde_json::json!({
        "manager": crate::MANAGER_VERSION,
        "core": meta.core,
        "bots": meta.bots,
        "kind": meta.kind,
        "layout": meta.layout,
        "checks": report.checks,
        "wow_client_set": meta.client_path.is_some(),
    });
    add("summary.json", serde_json::to_string_pretty(&summary)?.as_bytes())?;
    for (name, path) in [("manager.log", manager_log.to_path_buf()), ("manager-install.log", meta_dir.join("logs/manager.log")), ("Server.log.tail", root.join("Core/Logs/Server.log")), ("Errors.log.tail", root.join("Core/Logs/Errors.log")), ("Auth.log.tail", root.join("Core/Logs/Auth.log")), ("world-console.log.tail", root.join("Core/Logs/world-console.log")), ("mysql-error.log.tail", root.join("mysql/logs/mysql-error.log"))] {
        // A module that reads a missing setting on every tick fills a log with one warning, thousands of times; read far
        // enough back to see past that, fold the repeats, then keep the last part.
        let bytes = tail(&path, 8 * 1024 * 1024);
        if !bytes.is_empty() {
            let text = squash_repeated_config_warnings(&String::from_utf8_lossy(&bytes));
            let keep = text.len().saturating_sub(512 * 1024);
            let start = (keep..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
            add(name, redact(&text[start..]).as_bytes())?;
        }
    }
    // Which files the server's own configuration folders hold, and the two CoA switches that decide whether the game
    // client can talk to the world server at all.
    let mut present = String::new();
    for dir in ["Core/configs", "Core/configs/modules", "Core/Logs"] {
        if let Ok(rd) = fs::read_dir(root.join(dir)) {
            let mut names: Vec<String> = rd.flatten().filter(|e| e.path().is_file()).map(|e| format!("{dir}/{} ({} bytes)", e.file_name().to_string_lossy(), e.metadata().map(|m| m.len()).unwrap_or(0))).collect();
            names.sort();
            present.push_str(&names.join("\n"));
            present.push('\n');
        }
    }
    add("files.txt", present.as_bytes())?;
    if let Ok(b) = fs::read(root.join("Core/configs/modules/coa.conf")) {
        if let Ok(c) = ConfFile::parse_bytes(&b) {
            let wanted = ["CoA.Enable", "CoA.AllowRemoteClients", "CoA.MapClass10ToWarrior"];
            let lines: Vec<String> = c.entries().filter(|(k, _)| wanted.contains(k)).map(|(k, v)| format!("{k} = {v}")).collect();
            add("coa.conf.txt", lines.join("\n").as_bytes())?;
        }
    }
    // Which settings exist, never their values.
    if let Ok(b) = fs::read(root.join("Core/configs/worldserver.conf")) {
        if let Ok(c) = ConfFile::parse_bytes(&b) {
            let keys: Vec<&str> = c.entries().map(|(k, _)| k).collect();
            add("worldserver.conf.keys.txt", keys.join("\n").as_bytes())?;
        }
    }
    if let Ok(b) = fs::read(root.join("RELEASE.json")) {
        add("RELEASE.json", redact(&String::from_utf8_lossy(&b)).as_bytes())?;
    }
    if let Ok(b) = fs::read(meta_dir.join("logs/database-checks.json")) {
        add("database-checks.json", redact(&String::from_utf8_lossy(&b)).as_bytes())?;
    }
    zip.finish().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(n)
}

pub fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

pub fn desktop_or_temp() -> PathBuf {
    let d = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Desktop");
    if d.is_dir() { d } else { std::env::temp_dir() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{InstallKind, InstallMeta};

    #[test]
    fn repeated_config_warnings_are_folded() {
        let t = "start
> Config: Missing property A.B in config file x or module config, add y
> Config: Missing property A.B in config file x
real error
> Config: Missing property C.D in config file x
> Config: Missing property A.B in config file x";
        let out = squash_repeated_config_warnings(t);
        assert_eq!(out.matches("Missing property A.B in config file").count(), 1);
        assert!(out.contains("real error") && out.contains("C.D"));
        assert!(out.contains("\"Missing property A.B\" was logged 3 times"));
        assert!(!out.contains("C.D\" was logged"));
    }

    #[test]
    fn redaction_removes_lines_that_could_carry_secrets() {
        let t = redact("normal line\nLoginDatabaseInfo = \"127.0.0.1;3307;acore;PASS;db\"\nRa.Password=abc\nother");
        assert!(t.contains("normal line") && t.contains("other"));
        assert!(!t.contains("PASS") && !t.contains("abc"));
    }

    #[test]
    fn verify_reports_missing_and_changed_managed_files_only() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("ok.txt"), "a").unwrap();
        fs::write(d.path().join("changed.txt"), "b").unwrap();
        fs::write(d.path().join("user.txt"), "mine").unwrap();
        let mut meta = InstallMeta::new(InstallKind::New, d.path());
        meta.original_hashes.insert("ok.txt".into(), fsx::sha256_bytes(b"a"));
        meta.original_hashes.insert("changed.txt".into(), fsx::sha256_bytes(b"original"));
        meta.original_hashes.insert("gone.txt".into(), fsx::sha256_bytes(b"x"));
        let mut p: Vec<(String, &str)> = verify_managed(d.path(), &meta).into_iter().map(|f| (f.path, f.kind)).collect();
        p.sort();
        assert_eq!(p, [("changed.txt".to_string(), "changed"), ("gone.txt".to_string(), "missing")]);
    }

    #[test]
    fn diagnostics_on_a_fake_repack_flag_the_stopped_services_and_pass_the_basics() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        layout::testkit::fake_repack(&root);
        let meta = InstallMeta::new(InstallKind::Imported, &root);
        let r = run(&root, &meta);
        let get = |id: &str| r.checks.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!(get("files").level, Level::Ok);
        assert_eq!(get("permissions").level, Level::Ok);
        assert_eq!(get("configs").level, Level::Ok);
        assert_ne!(get("world").level, Level::Ok, "a stopped server is reported, not hidden");
        assert!(r.problems >= 3);
    }

    #[test]
    fn exported_package_holds_no_secrets() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        layout::testkit::fake_repack(&root);
        fs::write(root.join("Core/Logs/Errors.log"), "boom\nDatabase password=hunter2 rejected\n").unwrap();
        fs::write(root.join("Core/configs/worldserver.conf"), "LoginDatabaseInfo = \"127.0.0.1;3307;acore;SECRETPW;auth\"\nRate.XP.Kill = 1\n").unwrap();
        let meta_dir = d.path().join("srv.manager");
        fs::create_dir_all(&meta_dir).unwrap();
        let meta = InstallMeta::new(InstallKind::Imported, &root);
        let report = run(&root, &meta);
        let zip_path = d.path().join("diag.zip");
        let n = export_package(&root, &meta_dir, &d.path().join("none.log"), &meta, &report, &zip_path).unwrap();
        assert!(n >= 3);
        let mut z = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
        let mut all = String::new();
        for i in 0..z.len() {
            let mut s = String::new();
            let _ = z.by_index(i).unwrap().read_to_string(&mut s);
            all.push_str(&s);
        }
        assert!(all.contains("boom") && all.contains("Rate.XP.Kill"), "keys and ordinary log lines are included");
        assert!(!all.contains("hunter2") && !all.contains("SECRETPW"));
    }
}
