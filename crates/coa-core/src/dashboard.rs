//! SQUID's bot dashboard (<https://github.com/Zyth45/squidbots-dashboard>): a small web page, written in Python with
//! no dependencies, that shows where the bots are and what they do. It is kept in `Extras/SquidDashboard` of the server
//! folder and always comes from the release that matches the installed SQUID bots (same major and minor version), so the
//! page and the bots agree on what the database and the logs contain.
//!
//! The dashboard finds the repack by itself (`Settings/database.json` and `Settings/repack.json` two folders up), so the
//! Manager only has to download it and start it on a free port. Only the process the Manager started and recorded is ever
//! stopped, identified by PID and creation time.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;
use crate::process::{self, ProcessIdentity};

const REPO: &str = "Zyth45/squidbots-dashboard";
const DIR: &str = "Extras/SquidDashboard";
const MARKER: &str = ".manager-dashboard.json";
const PROCESS: &str = ".manager-dashboard-process.json";
const FIRST_PORT: u16 = 8088;
const MAX_ARCHIVE: u64 = 60 * 1024 * 1024;
const MAX_UNPACKED: u64 = 150 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    /// The dashboard release that is installed.
    tag: String,
    /// The SQUID release it was chosen for.
    squid_tag: String,
    port: u16,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub installed: bool,
    /// Dashboard release installed.
    pub tag: Option<String>,
    /// SQUID bots release found in the server.
    pub squid_tag: Option<String>,
    /// The installed dashboard belongs to the installed bots' release series.
    pub matches: bool,
    pub running: bool,
    pub port: u16,
    pub url: String,
}

fn dir(root: &Path) -> PathBuf {
    root.join(DIR)
}

fn read_marker(root: &Path) -> Option<Marker> {
    fsx::read_json(&dir(root).join(MARKER)).ok()
}

/// `v1.9.1` -> `[1, 9, 1]`.
fn parse_tag(tag: &str) -> Option<Vec<u32>> {
    let parts: Vec<u32> = tag
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    (parts.len() >= 2).then_some(parts)
}

/// Same major and minor version.
fn same_series(a: &str, b: &str) -> bool {
    matches!((parse_tag(a), parse_tag(b)), (Some(a), Some(b)) if a[..2] == b[..2])
}

/// The newest dashboard release of the bots' series (`v1.9` serves bots `v1.9` and `v1.9.1`).
fn pick_tag(squid_tag: &str, available: &[String]) -> Option<String> {
    available
        .iter()
        .filter(|tag| same_series(tag, squid_tag))
        .max_by_key(|tag| parse_tag(tag).unwrap_or_default())
        .cloned()
}

fn safe_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 40
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn squid_tag(root: &Path) -> Option<String> {
    crate::squid::release(root)
        .and_then(|r| r.tag)
        .filter(|t| safe_tag(t))
}

fn record_alive(root: &Path) -> Option<ProcessIdentity> {
    let record: ProcessIdentity = fsx::read_json(&dir(root).join(PROCESS)).ok()?;
    let inside = Path::new(&record.exe).starts_with(root);
    (inside && process::is_alive(&record)).then_some(record)
}

pub fn status(root: &Path) -> Status {
    let marker = read_marker(root);
    let installed = dir(root).join("squidbots.py").is_file() && marker.is_some();
    let squid = squid_tag(root);
    let port = marker.as_ref().map(|m| m.port).unwrap_or(FIRST_PORT);
    let running = installed
        && record_alive(root).is_some_and(|r| {
            process::listeners()
                .iter()
                .any(|(p, pid)| *p == port && *pid == r.pid)
        });
    Status {
        installed,
        matches: matches!((&marker, &squid), (Some(m), Some(s)) if same_series(&m.tag, s)),
        tag: marker.map(|m| m.tag),
        squid_tag: squid,
        running,
        port,
        url: format!("http://127.0.0.1:{port}"),
    }
}

fn http() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("CoA-Server-Manager/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| Error::Invalid(format!("Cannot prepare the download: {e}")))
}

fn download_error(e: impl std::fmt::Display) -> Error {
    Error::NetworkUnreachable(e.to_string())
}

fn available_tags(client: &reqwest::blocking::Client) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Tag {
        name: String,
    }
    let response = client
        .get(format!(
            "https://api.github.com/repos/{REPO}/tags?per_page=100"
        ))
        .send()
        .map_err(download_error)?;
    if !response.status().is_success() {
        return Err(download_error(format!(
            "GitHub answered {}",
            response.status()
        )));
    }
    let tags: Vec<Tag> =
        serde_json::from_str(&response.text().map_err(download_error)?).map_err(download_error)?;
    Ok(tags
        .into_iter()
        .map(|t| t.name)
        .filter(|t| safe_tag(t))
        .collect())
}

/// Download the dashboard release that matches the installed bots and unpack it. A running dashboard is stopped first;
/// maps the owner extracted and other files of theirs are left alone.
pub fn install(root: &Path, report: &dyn Fn(&str)) -> Result<Status> {
    let squid = squid_tag(root).ok_or_else(|| Error::Invalid("The version of the SQUID bots could not be read, so the matching dashboard cannot be chosen.".into()))?;
    report("Looking for the matching dashboard");
    let client = http()?;
    let tag = pick_tag(&squid, &available_tags(&client)?).ok_or_else(|| {
        Error::Invalid(format!(
            "There is no dashboard release for SQUID bots {squid} yet."
        ))
    })?;
    report("Downloading the dashboard");
    let response = client
        .get(format!(
            "https://codeload.github.com/{REPO}/zip/refs/tags/{tag}"
        ))
        .send()
        .map_err(download_error)?;
    if !response.status().is_success() {
        return Err(download_error(format!(
            "GitHub answered {}",
            response.status()
        )));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_ARCHIVE + 1)
        .read_to_end(&mut bytes)
        .map_err(download_error)?;
    if bytes.len() as u64 > MAX_ARCHIVE {
        return Err(Error::Invalid(
            "The dashboard download is larger than expected and was rejected.".into(),
        ));
    }

    stop(root)?;
    report("Unpacking the dashboard");
    let target = dir(root);
    std::fs::create_dir_all(&target)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| Error::Invalid(format!("The dashboard download is damaged: {e}")))?;
    let mut total = 0u64;
    let mut wrote_main = false;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| Error::Invalid(format!("The dashboard download is damaged: {e}")))?;
        if file.is_dir() {
            continue;
        }
        // GitHub wraps everything in one folder named after the repository and release.
        let Some(relative) = file
            .name()
            .split_once('/')
            .map(|(_, rest)| rest.to_string())
        else {
            continue;
        };
        if relative.is_empty() || relative.starts_with(".github/") || relative.starts_with("docs/")
        {
            continue;
        }
        total += file.size();
        if total > MAX_UNPACKED {
            return Err(Error::Invalid(
                "The dashboard download unpacks to more than expected and was rejected.".into(),
            ));
        }
        let destination = fsx::safe_join(&target, &relative)?;
        let mut content = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut content)?;
        fsx::atomic_write(&destination, &content)?;
        wrote_main |= relative == "squidbots.py";
    }
    if !wrote_main {
        return Err(Error::Invalid(
            "The dashboard download did not contain the dashboard.".into(),
        ));
    }
    let port = read_marker(root).map(|m| m.port).unwrap_or(FIRST_PORT);
    fsx::atomic_write_json(
        &target.join(MARKER),
        &Marker {
            tag,
            squid_tag: squid,
            port,
        },
    )?;
    Ok(status(root))
}

fn free_port(preferred: u16) -> Result<u16> {
    let used: Vec<u16> = process::listeners()
        .into_iter()
        .map(|(port, _)| port)
        .collect();
    std::iter::once(preferred)
        .chain(FIRST_PORT..FIRST_PORT + 40)
        .find(|p| !used.contains(p))
        .ok_or_else(|| Error::Invalid("No free port was found for the dashboard.".into()))
}

/// A crash or power loss while the dashboard saves its history can leave a file of the right length that holds only
/// zero bytes, and the dashboard then fails to start on it. Those files carry nothing, and the dashboard recreates them,
/// so they are removed. The files that come with the release are never touched here (reinstalling restores them).
fn discard_damaged_data(folder: &Path) -> Vec<String> {
    const SHIPPED: &[&str] = &["worldmap.json", "dashboard.example.json"];
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return removed;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if !name.ends_with(".json")
            || name.starts_with('.')
            || SHIPPED.contains(&name.as_str())
            || !path.is_file()
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if !bytes.is_empty() && bytes.iter().all(|b| *b == 0) && std::fs::remove_file(&path).is_ok()
        {
            tracing::warn!(file = %path.display(), "dashboard: removed a data file damaged by an interrupted write");
            removed.push(name);
        }
    }
    removed
}

/// The dashboard reads `coa-level-builds.json` (class and specialization names) from its own folder; the CoA Bots repack
/// ships it, the GitHub release does not. The server's own reference file has the same content and shape.
fn ensure_builds_file(root: &Path) {
    let target = dir(root).join("coa-level-builds.json");
    if target.is_file() {
        return;
    }
    let reference = root.join("Core/reference/ascensionsidekick-level-builds.json");
    let content = std::fs::read(&reference).ok().filter(|bytes| {
        serde_json::from_slice::<serde_json::Value>(bytes).is_ok_and(|v| v.is_object())
    });
    let _ = fsx::atomic_write(&target, content.as_deref().unwrap_or(b"{}"));
}

/// Python's HTTP server closes each connection with `shutdown(SHUT_WR)` straight after the answer. On Windows that can
/// throw away what is still queued: any answer above about 64 KB (the dashboard's statistics are 400 KB) arrives cut
/// off, and the page fails to load. This small launcher waits for the client to finish reading and close instead, queues
/// more waiting connections than Python's five, and then runs the dashboard unchanged.
const LAUNCHER: &str = r#"import runpy, socketserver, sys, http.server

http.server.ThreadingHTTPServer.request_queue_size = 256


def shutdown_request(self, request):
    try:
        request.settimeout(5)
        while request.recv(4096):
            pass
    except OSError:
        pass
    self.close_request(request)


socketserver.TCPServer.shutdown_request = shutdown_request
script = sys.argv[1]
sys.argv = [script]
runpy.run_path(script, run_name="__main__")
"#;

/// Start the dashboard with the repack's own Python. Returns when it answers on its port.
pub fn start(root: &Path) -> Result<Status> {
    let current = status(root);
    if !current.installed {
        return Err(Error::Invalid("The dashboard is not installed.".into()));
    }
    if current.running {
        return Ok(current);
    }
    let python = root.join("Runtime/python/python.exe");
    if !python.is_file() {
        return Err(Error::Invalid(
            "This server has no bundled Python, so the dashboard cannot run.".into(),
        ));
    }
    let folder = dir(root);
    let mut marker = read_marker(root)
        .ok_or_else(|| Error::Invalid("The dashboard installation is incomplete.".into()))?;
    marker.port = free_port(marker.port)?;
    fsx::atomic_write_json(&folder.join(MARKER), &marker)?;

    let log_path = folder.join("dashboard.log");
    let log = std::fs::File::create(&log_path)?;
    let mut command = std::process::Command::new(&python);
    ensure_builds_file(root);
    discard_damaged_data(&folder);
    let script = folder.join("squidbots.py");
    let launcher = folder.join("manager_launch.py");
    fsx::atomic_write(&launcher, LAUNCHER.as_bytes())?;
    command
        .arg("-B")
        .arg(&launcher)
        .arg(&script)
        .current_dir(&folder)
        .env("COA_DASHBOARD_PORT", marker.port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn()?;
    let identity = process::identity(child.id())
        .ok_or_else(|| Error::Invalid("The dashboard exited at once.".into()))?;
    fsx::atomic_write_json(&folder.join(PROCESS), &identity)?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if process::listeners()
            .iter()
            .any(|(port, pid)| *port == marker.port && *pid == identity.pid)
        {
            return Ok(status(root));
        }
        if let Ok(Some(_)) = child.try_wait() {
            let _ = std::fs::remove_file(folder.join(PROCESS));
            let tail = std::fs::read_to_string(&log_path).unwrap_or_default();
            let lines: Vec<&str> = tail.lines().rev().take(8).collect();
            return Err(Error::Invalid(format!(
                "The dashboard stopped right after starting. {}",
                lines.into_iter().rev().collect::<Vec<_>>().join("\n")
            )));
        }
        if std::time::Instant::now() > deadline {
            let _ = stop(root);
            return Err(Error::Invalid(
                "The dashboard did not start in time. See Extras/SquidDashboard/dashboard.log."
                    .into(),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}

/// Stop the dashboard this Manager started (by recorded PID and creation time, never by name).
pub fn stop(root: &Path) -> Result<()> {
    let record_file = dir(root).join(PROCESS);
    if let Some(record) = record_alive(root) {
        let mut command = std::process::Command::new("taskkill");
        command
            .args(["/PID", &record.pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let _ = command.status();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while process::is_alive(&record) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if process::is_alive(&record) {
            return Err(Error::Invalid("The dashboard could not be stopped.".into()));
        }
    }
    let _ = std::fs::remove_file(record_file);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_dashboard_follows_the_bots_release_series() {
        let available = tags(&["v1.8", "v1.9", "v1.9.2", "v2.0"]);
        assert_eq!(
            pick_tag("v1.9", &available).as_deref(),
            Some("v1.9.2"),
            "the newest of the same series"
        );
        assert_eq!(
            pick_tag("v1.9.1", &tags(&["v1.8", "v1.9"])).as_deref(),
            Some("v1.9"),
            "bots 1.9.1 use dashboard 1.9"
        );
        assert_eq!(pick_tag("v1.8.1", &available).as_deref(), Some("v1.8"));
        assert_eq!(
            pick_tag("v1.10", &available),
            None,
            "a later series has none yet"
        );
        assert_eq!(pick_tag("main", &available), None);
        assert!(same_series("v1.9", "v1.9.1") && !same_series("v1.9", "v1.8.9"));
    }

    #[test]
    fn zero_filled_history_files_are_removed_but_shipped_and_valid_files_stay() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("history.json"), vec![0u8; 358]).unwrap();
        std::fs::write(folder.path().join("levels.json"), vec![0u8; 100]).unwrap();
        std::fs::write(folder.path().join("xp-history.json"), "[1, 2]").unwrap();
        std::fs::write(folder.path().join("worldmap.json"), vec![0u8; 50]).unwrap();
        std::fs::write(folder.path().join("empty.json"), "").unwrap();
        let mut removed = discard_damaged_data(folder.path());
        removed.sort();
        assert_eq!(removed, ["history.json", "levels.json"]);
        assert!(
            folder.path().join("xp-history.json").exists()
                && folder.path().join("worldmap.json").exists()
                && folder.path().join("empty.json").exists()
        );
    }

    #[test]
    fn tags_are_checked_before_they_reach_a_url() {
        assert!(safe_tag("v1.9.1") && !safe_tag("v1.9/../x") && !safe_tag("") && !safe_tag("a b"));
    }

    /// Needs the network: `cargo test -p coa-core dashboard -- --ignored`.
    #[test]
    #[ignore]
    fn the_matching_release_is_downloaded_and_unpacked() {
        let root = tempfile::tempdir().unwrap();
        let settings = root
            .path()
            .join("Core/configs/modules/playerbots.conf.settings.json");
        fsx::atomic_write(&settings, br#"{"format":1,"tag":"v1.9.1","settings":[]}"#).unwrap();
        let installed = install(root.path(), &|step| println!("{step}")).unwrap();
        assert!(installed.installed && installed.matches);
        assert_eq!(installed.tag.as_deref(), Some("v1.9"));
        assert!(root
            .path()
            .join("Extras/SquidDashboard/squidbots.py")
            .is_file());
        assert!(!installed.running);
    }

    /// Needs a real server folder (`COA_TEST_ROOT`) whose bots are installed and whose database runs:
    /// `COA_TEST_ROOT=... cargo test -p coa-core dashboard -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn the_started_dashboard_delivers_its_large_statistics() {
        let root = PathBuf::from(std::env::var("COA_TEST_ROOT").expect("COA_TEST_ROOT"));
        let started = start(&root).unwrap();
        assert!(started.running, "{started:?}");
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(90))
            .build()
            .unwrap();
        let body = client
            .get(format!("{}/api/stats", started.url))
            .send()
            .unwrap()
            .bytes()
            .unwrap();
        println!(
            "stats: {} bytes: {}",
            body.len(),
            String::from_utf8_lossy(&body[..body.len().min(300)])
        );
        let live = client
            .get(format!("{}/api/live", started.url))
            .send()
            .unwrap()
            .bytes()
            .unwrap();
        stop(&root).unwrap();
        assert!(!status(&root).running);
        if String::from_utf8_lossy(&body).starts_with("{\"error\"") {
            println!("the database of this server is not running, so the large answer could not be checked");
        } else {
            assert!(
                body.len() > 100_000,
                "the statistics must arrive whole, got {} bytes",
                body.len()
            );
        }
        assert!(!live.is_empty());
    }

    #[test]
    fn a_folder_without_the_dashboard_is_not_installed_or_running() {
        let root = tempfile::tempdir().unwrap();
        let s = status(root.path());
        assert!(!s.installed && !s.running && !s.matches);
        assert!(start(root.path()).is_err());
        assert!(stop(root.path()).is_ok());
    }
}
