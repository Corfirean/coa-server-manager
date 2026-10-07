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

const SENSITIVE: [&str; 7] = ["password", "passwd", "secret", "token", "apikey", "api_key", "databaseinfo"];

/// Remove things that must never leave the machine from log text.
pub fn redact(text: &str) -> String {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.to_lowercase();
        let sensitive = SENSITIVE.iter().any(|k| l.contains(k));
        out.push(if sensitive { "[line removed: may contain a secret]".to_string() } else { line.to_string() });
    }
    out.join("\n")
}

/// The same for JSON: values under a sensitive key and strings that look like `password=...` are replaced, and the
/// result stays valid JSON (removing a whole line would break the file for whoever reads it). Text that is not JSON
/// falls back to the line rule.
pub fn redact_json(text: &str) -> String {
    fn scrub(v: &mut serde_json::Value) {
        use serde_json::Value;
        match v {
            Value::Object(map) => {
                for (key, value) in map.iter_mut() {
                    let k = key.to_lowercase();
                    if SENSITIVE.iter().any(|s| k.contains(s)) && !value.is_object() && !value.is_array() {
                        *value = Value::String("[redacted]".into());
                    } else {
                        scrub(value);
                    }
                }
            }
            Value::Array(items) => items.iter_mut().for_each(scrub),
            Value::String(s) => {
                let l = s.to_lowercase();
                if ["password=", "passwd=", "secret=", "token=", "apikey=", "api_key=", "password:"].iter().any(|k| l.contains(k)) {
                    *s = "[redacted]".into();
                }
            }
            _ => {}
        }
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(mut v) => {
            scrub(&mut v);
            serde_json::to_string_pretty(&v).unwrap_or_else(|_| redact(text))
        }
        Err(_) => redact(text),
    }
}

/// Keep the events of a `wevtutil ... /f:text` listing that mention one of `needles` (compared without case), in the
/// order listed, at most `max` of them.
pub fn filter_event_blocks(listing: &str, needles: &[&str], max: usize) -> String {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in listing.lines() {
        if line.starts_with("Event[") || blocks.is_empty() {
            blocks.push(Vec::new());
        }
        if let Some(b) = blocks.last_mut() { b.push(line); }
    }
    blocks
        .into_iter()
        .map(|b| b.join("\n"))
        .filter(|b| { let l = b.to_lowercase(); needles.iter().any(|n| l.contains(n)) })
        .take(max)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Drop the lines of an event listing that name the person or the computer.
pub fn drop_identity_lines(listing: &str) -> String {
    listing
        .lines()
        .filter(|l| { let t = l.trim_start(); !(t.starts_with("User:") || t.starts_with("User Name:") || t.starts_with("Computer:")) })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `wevtutil /uni:true` writes UTF-16 with a byte order mark; anything else is read as UTF-8 (lossy).
fn decode_console_output(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Crashes and hangs Windows recorded for the server programs in the last two weeks. This is the only trace of a crash
/// that ends the process without the server's own crash report (a stack buffer overrun, for example). The lines that
/// name the user and the computer are left out.
#[cfg(windows)]
fn windows_events() -> String {
    use std::os::windows::process::CommandExt;
    let query = "*[System[(Provider[@Name='Application Error' or @Name='Windows Error Reporting' or @Name='Application Hang']) and TimeCreated[timediff(@SystemTime) <= 1209600000]]]";
    let out = std::process::Command::new("wevtutil")
        .args(["qe", "Application", &format!("/q:{query}"), "/c:300", "/rd:true", "/f:text", "/uni:true"])
        .creation_flags(0x0800_0000)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = filter_event_blocks(&decode_console_output(&o.stdout), &["worldserver", "authserver", "mysqld", "coa server manager", "coa-server-manager"], 40);
            if text.is_empty() { "No crash or hang of the server programs was recorded by Windows in the last 14 days.".into() } else { drop_identity_lines(&text) }
        }
        Ok(o) => format!("Windows event log could not be read: {}", decode_console_output(&o.stderr).trim()),
        Err(e) => format!("Windows event log could not be read: {e}"),
    }
}

#[cfg(not(windows))]
fn windows_events() -> String {
    String::new()
}

/// Newest files of a folder with the given extension (compared without case): path, size, modified time.
fn newest_files(dir: &Path, ext: &str, limit: usize) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<_> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
        .filter_map(|e| { let m = e.metadata().ok()?; m.is_file().then(|| (e.path(), m.len(), m.modified().unwrap_or(std::time::UNIX_EPOCH))) })
        .collect();
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v.truncate(limit);
    v
}

/// Which server programs run right now and from where, next to what the Manager made of it. A program started from a
/// folder or a copy the Manager does not know shows up here even when the Manager reports "port in use".
fn processes_report(root: &Path, ports: &layout::Ports) -> String {
    let here = root.to_string_lossy().to_lowercase().replace('/', "\\");
    let listen = process::listeners_detailed();
    let mut out = String::from("Server programs running now (any folder):\n");
    let found = process::find_by_file_names(&["worldserver.exe", "authserver.exe", "mysqld.exe"]);
    if found.is_empty() { out.push_str("  none\n"); }
    for p in found {
        let ports_of: Vec<String> = listen.iter().filter(|l| l.pid == p.pid).map(|l| l.port.to_string()).collect();
        let inside = p.exe.to_lowercase().replace('/', "\\").starts_with(&here);
        out.push_str(&format!("  pid {} {} | listening on: {} | {}\n", p.pid, p.exe, if ports_of.is_empty() { "-".into() } else { ports_of.join(", ") }, if inside { "inside this server folder" } else { "OUTSIDE this server folder" }));
    }
    let o = process::observe(root, ports);
    out.push_str("\nThe Manager's view:\n");
    for s in [Some(&o.mysql), Some(&o.auth), Some(&o.world), o.secondary_world.as_ref()].into_iter().flatten() {
        out.push_str(&format!("  {:<16} {:?} port {} {}{}\n", s.name, s.state, s.port, s.pid.map(|p| format!("pid {p}")).unwrap_or_default(), s.conflict.as_ref().map(|c| format!(" | port held by pid {} {}", c.pid, c.exe.clone().unwrap_or_default())).unwrap_or_default()));
    }
    out
}

/// Zip with what a maintainer needs to debug a problem, with secrets removed. Returns the number of files inside.
/// One call collects everything: the Manager's and the server's logs, crash reports and small crash dumps, Windows'
/// own record of crashes, the update journals, the running server programs and the settings the client depends on.
pub fn export_package(root: &Path, meta_dir: &Path, manager_log: &Path, meta: &InstallMeta, report: &Report, out_zip: &Path) -> Result<usize> {
    const DUMP_FILE_MAX: u64 = 30 * 1024 * 1024;
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
    let ports = layout::read_ports(root);
    let summary = serde_json::json!({
        "manager": crate::MANAGER_VERSION,
        "exported_utc": chrono::Utc::now().to_rfc3339(),
        "core": meta.core,
        "bots": meta.bots,
        "squid_bots": crate::squid::release(root),
        "kind": meta.kind,
        "layout": meta.layout,
        "checks": report.checks,
        "wow_client_set": meta.client_path.is_some(),
    });
    add("summary.json", serde_json::to_string_pretty(&summary)?.as_bytes())?;

    // Logs. The newest update logs of the repack's own updater are added under their own names.
    let mut logs: Vec<(String, PathBuf)> = vec![
        ("manager.log".into(), manager_log.to_path_buf()),
        ("manager-install.log".into(), meta_dir.join("logs/manager.log")),
        ("Server.log.tail".into(), root.join("Core/Logs/Server.log")),
        ("Errors.log.tail".into(), root.join("Core/Logs/Errors.log")),
        ("Auth.log.tail".into(), root.join("Core/Logs/Auth.log")),
        ("world-console.log.tail".into(), root.join("Core/Logs/world-console.log")),
        ("CoaBots.log.tail".into(), root.join("Core/Logs/CoaBots.log")),
        ("Playerbots.log.tail".into(), root.join("Core/Logs/Playerbots.log")),
        ("supervisor.log.tail".into(), root.join("Core/Logs/supervisor.log")),
        ("mysql-error.log.tail".into(), root.join("mysql/logs/mysql-error.log")),
    ];
    let updater_logs = newest_files(&root.join("Core/Logs"), "log", usize::MAX).into_iter().filter(|(p, _, _)| p.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase().starts_with("update-")));
    for (p, _, _) in updater_logs.take(2) {
        logs.push((format!("{}.tail", p.file_name().unwrap_or_default().to_string_lossy()), p));
    }
    for (name, path) in logs {
        // A module that reads a missing setting on every tick fills a log with one warning, thousands of times; read far
        // enough back to see past that, fold the repeats, then keep the last part.
        let bytes = tail(&path, 8 * 1024 * 1024);
        if !bytes.is_empty() {
            let text = squash_repeated_config_warnings(&String::from_utf8_lossy(&bytes));
            let keep = text.len().saturating_sub(512 * 1024);
            let start = (keep..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
            add(&name, redact(&text[start..]).as_bytes())?;
        }
    }

    // Crash reports of the server (text, and the small dumps next to them), and what Windows recorded for the programs.
    let crashes = root.join("Core/Crashes");
    let mut listing = String::new();
    for (p, len, _) in newest_files(&crashes, "txt", 10) {
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        listing.push_str(&format!("{name} ({len} bytes)\n"));
        let text = String::from_utf8_lossy(&tail(&p, 1024 * 1024)).into_owned();
        add(&format!("crashes/{name}"), redact(&text).as_bytes())?;
    }
    for (p, len, _) in newest_files(&crashes, "dmp", 3) {
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if len > DUMP_FILE_MAX {
            listing.push_str(&format!("{name} ({len} bytes) not included: larger than {} MB\n", DUMP_FILE_MAX >> 20));
        } else if let Ok(bytes) = fs::read(&p) {
            listing.push_str(&format!("{name} ({len} bytes) included\n"));
            add(&format!("crashes/{name}"), &bytes)?;
        }
    }
    add("crashes.txt", if listing.is_empty() { "No crash reports in Core/Crashes.".to_string() } else { listing }.as_bytes())?;
    let events = windows_events();
    if !events.is_empty() { add("windows-events.txt", events.as_bytes())?; }
    add("processes.txt", processes_report(root, &ports).as_bytes())?;

    // The newest update journals: the state of each update and the reason it failed.
    let mut journals: Vec<_> = fs::read_dir(meta_dir.join("updates")).into_iter().flatten().flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && e.file_name().to_string_lossy().chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        .collect();
    journals.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    for e in journals.into_iter().take(5) {
        if let Ok(text) = fs::read_to_string(e.path().join("txn.json")) {
            add(&format!("updates/{}-txn.json", e.file_name().to_string_lossy()), redact_json(&text).as_bytes())?;
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
        add("RELEASE.json", redact_json(&String::from_utf8_lossy(&b)).as_bytes())?;
    }
    if let Ok(b) = fs::read(meta_dir.join("logs/database-checks.json")) {
        add("database-checks.json", redact_json(&String::from_utf8_lossy(&b)).as_bytes())?;
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
    fn json_redaction_keeps_the_file_valid_and_hides_the_secrets() {
        let t = redact_json(r#"{"id":"rev_20261006_token_cache","password":"hunter2","nested":{"apiKey":"k","note":"login with password=abc"},"list":[{"secret":1},"ok"]}"#);
        let v: serde_json::Value = serde_json::from_str(&t).expect("still JSON");
        assert_eq!(v["id"], "rev_20261006_token_cache", "an id that merely contains a word is kept");
        assert_eq!(v["password"], "[redacted]");
        assert_eq!(v["nested"]["apiKey"], "[redacted]");
        assert_eq!(v["nested"]["note"], "[redacted]");
        assert_eq!(v["list"][0]["secret"], "[redacted]");
        assert_eq!(v["list"][1], "ok");
        assert!(!t.contains("hunter2") && !t.contains("abc"));
        assert!(redact_json("not json\nPassword=1").contains("[line removed"), "text falls back to the line rule");
    }

    #[test]
    fn windows_events_are_filtered_to_the_server_programs() {
        let listing = "Event[0]:\n  Provider Name: Application Error\n  Description: Faulting application name: worldserver.exe, version: 0.0.0.0\n\nEvent[1]:\n  Provider Name: Application Error\n  Description: Faulting application name: notepad.exe\n\nEvent[2]:\n  Description: Faulting application name: MYSQLD.EXE\n";
        let out = filter_event_blocks(listing, &["worldserver", "mysqld"], 10);
        assert!(out.contains("worldserver.exe") && out.contains("MYSQLD.EXE") && !out.contains("notepad"));
        assert_eq!(filter_event_blocks(listing, &["worldserver", "mysqld"], 1).matches("Event[").count(), 1);
    }

    #[test]
    fn event_text_loses_the_user_and_computer_lines_and_reads_utf16() {
        let out = drop_identity_lines("Event[0]\n  Source: Application Error\n  User: S-1-5-21-1\n  User Name: PC\\Dion\n  Computer: PC\n  Description: worldserver.exe");
        assert!(out.contains("Source") && out.contains("worldserver.exe"));
        assert!(!out.contains("S-1-5") && !out.contains("Dion") && !out.contains("Computer"));
        let mut bytes = vec![0xFF, 0xFE];
        for u in "Сбой worldserver".encode_utf16() { bytes.extend_from_slice(&u.to_le_bytes()); }
        assert_eq!(decode_console_output(&bytes), "Сбой worldserver");
        assert_eq!(decode_console_output(b"plain"), "plain");
    }

    #[test]
    fn the_package_carries_crash_reports_update_journals_and_the_process_view() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        layout::testkit::fake_repack(&root);
        fs::create_dir_all(root.join("Core/Crashes")).unwrap();
        fs::write(root.join("Core/Crashes/567e_worldserver.exe_[4-10_21-25-31].txt"), "Exception code: C0000005\nRCX:0\n").unwrap();
        fs::write(root.join("Core/Crashes/567e_worldserver.exe_[4-10_21-25-31].dmp"), b"MDMP-small").unwrap();
        fs::write(root.join("Core/Logs/CoaBots.log"), "bot line\n").unwrap();
        fs::write(root.join("Core/Logs/update-1.8-20261005.log"), "updater line\n").unwrap();
        let meta_dir = d.path().join("srv.manager");
        fs::create_dir_all(meta_dir.join("updates/20261006-082021-0_261006_1-abc")).unwrap();
        fs::write(meta_dir.join("updates/20261006-082021-0_261006_1-abc/txn.json"), r#"{"state":"rolled-back","message":"Database update 2026_02_24_00 failed: duplicate column"}"#).unwrap();
        fs::create_dir_all(meta_dir.join("updates/not a journal!")).unwrap();
        let meta = InstallMeta::new(InstallKind::Imported, &root);
        let report = run(&root, &meta);
        let zip_path = d.path().join("diag.zip");
        export_package(&root, &meta_dir, &d.path().join("none.log"), &meta, &report, &zip_path).unwrap();
        let mut z = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).collect();
        for want in ["crashes.txt", "processes.txt", "CoaBots.log.tail", "update-1.8-20261005.log.tail", "updates/20261006-082021-0_261006_1-abc-txn.json", "crashes/567e_worldserver.exe_[4-10_21-25-31].txt", "crashes/567e_worldserver.exe_[4-10_21-25-31].dmp"] {
            assert!(names.iter().any(|n| n == want), "{want} is in the package: {names:?}");
        }
        assert!(!names.iter().any(|n| n.contains("not a journal")));
        let mut listing = String::new();
        z.by_name("crashes.txt").unwrap().read_to_string(&mut listing).unwrap();
        assert!(listing.contains("included"));
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
        fs::create_dir_all(root.join("Extras/SquidPlayerbots")).unwrap();
        fs::write(root.join("Extras/SquidPlayerbots/release.json"), r#"{"tag":"v1.8","commit":"48c4786a","password":"upstream-secret"}"#).unwrap();
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
        assert!(all.contains("v1.8") && all.contains("48c4786a"));
        assert!(!all.contains("hunter2") && !all.contains("SECRETPW") && !all.contains("upstream-secret"));
    }
}
