//! The bug-report relay (`BugReport/relay.py` and `manage.py relay-worker`): delivers the reports the world
//! writes to `BugReport/reports/*.report` through the CoA report service.
//!
//! On disk everything stays as the Python relay keeps it, so either can take over a spool: the `<key>.status`
//! files the world reads back, the delivery journal `relay.sqlite3` (tables `deliveries` and `settings`), and
//! `relay.lock` (byte 0 locked while a worker runs). A delivery interrupted while it was posting becomes
//! `uncertain` and is never sent again (the service's duplicate filter may have been reset in the meantime).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::launcher::state;

const REPOSITORY: &str = "jealous-sound/azerothcore-wotlk-coa";
pub const SERVICE_URL: &str = "https://coa-bug-report.up.railway.app";
/// Distribution credential for this narrow intake API (the same as relay.py's); not a GitHub token.
pub const DEFAULT_API_KEY: &str = "coa_zRatxUHShOn-HEkoreKGME6sd3DdMG7eKTTKlj2JCK4";
const MAX_FILE: u64 = 16000;
const MAX_RESPONSE: usize = 64 * 1024;

/// `[1-9][0-9]{0,9}-[0-9a-f]{16,48}`: the name of a report file (without `.report`).
pub fn valid_key(key: &str) -> bool {
    let Some((id, hex)) = key.split_once('-') else { return false };
    (1..=10).contains(&id.len())
        && !id.starts_with('0')
        && id.bytes().all(|b| b.is_ascii_digit())
        && (16..=48).contains(&hex.len())
        && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Why a delivery did not create an issue. Only fixed local codes: never service bodies, credentials or report
/// content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// 202 pending: the service is still creating it.
    Pending(u64),
    /// 429: try again later.
    Limited(u64),
    /// The report itself was refused.
    Invalid,
    /// The service refused this relay (redirect, auth, missing endpoint, GitHub access).
    Blocked(u64),
    /// The outcome is unknown (network, timeout, unexpected answer).
    Uncertain,
}

/// Where reports go.
pub trait Service {
    fn create(&self, title: &str, body: &str) -> std::result::Result<u64, Failure>;
}

/// The CoA report service over HTTPS.
pub struct ReportService {
    api: String,
    key: String,
    client: reqwest::blocking::Client,
}

impl ReportService {
    pub fn new(base_url: &str, api_key: &str) -> Result<ReportService> {
        if api_key.is_empty() || !api_key.is_ascii() || api_key.chars().any(char::is_whitespace) {
            return Err(Error::Invalid("Configure a valid report-service intake key.".into()));
        }
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            // One report every few seconds at most: a fresh connection each time, never a stale pooled one.
            .pool_max_idle_per_host(0)
            .user_agent("CoA-InGame-BugReport/1")
            .build()
            .map_err(|e| Error::Invalid(format!("The report client could not start: {e}")))?;
        Ok(ReportService { api: format!("{base_url}/v1/reports"), key: api_key.into(), client })
    }
}

/// `Retry-After` in seconds, kept within 1 s .. 1 day; `default` when absent or not a number.
fn retry_delay(retry_after: Option<&str>, default: u64) -> u64 {
    retry_after.and_then(|v| v.trim().parse::<i64>().ok()).map_or(default, |v| v.clamp(1, 86400) as u64)
}

/// A JSON object of at most 64 KiB, or nothing.
fn json_object(raw: Option<&[u8]>) -> Option<serde_json::Map<String, serde_json::Value>> {
    let raw = raw?;
    if raw.len() > MAX_RESPONSE {
        return None;
    }
    match serde_json::from_slice(raw).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

fn issue_number(data: &serde_json::Map<String, serde_json::Value>) -> std::result::Result<u64, Failure> {
    let number = data.get("issue_number").and_then(|n| if n.is_u64() { n.as_u64() } else { None });
    match number {
        Some(n)
            if data.get("status").and_then(|s| s.as_str()) == Some("created")
                && (1..=4294967295).contains(&n)
                && data.get("issue_url").and_then(|u| u.as_str())
                    == Some(format!("https://github.com/{REPOSITORY}/issues/{n}").as_str()) =>
        {
            Ok(n)
        }
        _ => Err(Failure::Uncertain),
    }
}

/// What a service answer means (relay.py's `create`): the status, its `Retry-After` and its body (`None` when it
/// could not be read or was longer than 64 KiB).
pub fn classify(status: u16, retry_after: Option<&str>, body: Option<&[u8]>) -> std::result::Result<u64, Failure> {
    let field = |data: &serde_json::Map<String, serde_json::Value>, k: &str| data.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    match status {
        200..=202 => {
            let data = json_object(body).ok_or(Failure::Uncertain)?;
            match status {
                202 if field(&data, "status").as_deref() == Some("pending") => Err(Failure::Pending(retry_delay(retry_after, 2))),
                202 => Err(Failure::Uncertain),
                _ => issue_number(&data),
            }
        }
        429 => Err(Failure::Limited(retry_delay(retry_after, 300))),
        400 | 413 | 415 | 422 => Err(Failure::Invalid),
        301 | 302 | 303 | 307 | 308 | 401 | 403 | 404 | 405 => Err(Failure::Blocked(300)),
        503 => {
            let data = json_object(body).unwrap_or_default();
            if field(&data, "status").as_deref() == Some("failed") && field(&data, "error").as_deref() == Some("github_access_denied") {
                Err(Failure::Blocked(retry_delay(retry_after, 300)))
            } else {
                Err(Failure::Uncertain)
            }
        }
        _ => Err(Failure::Uncertain),
    }
}

impl Service for ReportService {
    fn create(&self, title: &str, body: &str) -> std::result::Result<u64, Failure> {
        use std::io::Read;
        let sent = self
            .client
            .post(&self.api)
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .body(serde_json::json!({ "title": title, "body": body }).to_string())
            .send();
        let response = sent.map_err(|_| Failure::Uncertain)?;
        let status = response.status().as_u16();
        let retry_after = response.headers().get("Retry-After").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let mut raw = Vec::new();
        let read = response.take(MAX_RESPONSE as u64 + 1).read_to_end(&mut raw).is_ok() && raw.len() <= MAX_RESPONSE;
        classify(status, retry_after.as_deref(), read.then_some(raw.as_slice()))
    }
}

/// A report: `COABUG1\n<title>\n<body>`, checked as the world writes it.
pub struct Report {
    pub digest: String,
    pub marker: String,
    pub title: String,
    pub body: String,
}

pub fn read_report(path: &Path) -> Result<Report> {
    let bad = || Error::Invalid("Invalid report file".into());
    let key = path.file_stem().and_then(|s| s.to_str()).ok_or_else(bad)?;
    let meta = std::fs::symlink_metadata(path)?;
    if !valid_key(key) || meta.file_type().is_symlink() || meta.len() > MAX_FILE {
        return Err(bad());
    }
    let raw = std::fs::read(path)?;
    let text = String::from_utf8(raw.clone()).map_err(|_| bad())?.replace("\r\n", "\n");
    let rest = text.strip_prefix("COABUG1\n").ok_or_else(|| Error::Invalid("Unsupported report format".into()))?;
    let invalid = || Error::Invalid("Invalid report content".into());
    let (title, body) = rest.split_once('\n').ok_or_else(invalid)?;
    if !(3..=200).contains(&title.len())
        || body.trim().is_empty()
        || title.chars().any(|c| (c as u32) < 32)
        || body.chars().any(|c| (c as u32) < 32 && !matches!(c, '\r' | '\n' | '\t'))
    {
        return Err(invalid());
    }
    let digest = hex(&Sha256::digest(&raw));
    let id = hex(&Sha256::digest(format!("{key}:{digest}").as_bytes()));
    Ok(Report { digest, marker: format!("<!-- coa-report:{id} -->"), title: title.into(), body: body.into() })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `<key>.status`: what the world shows the player (`queued`, `created|<issue>`, `failed`, ...).
pub fn write_status(spool: &Path, key: &str, state: &str, number: Option<u64>) -> Result<()> {
    let value = match (state, number) {
        ("created", Some(n)) => format!("created|{n}"),
        ("created", None) => "created|None".into(),
        _ => state.to_string(),
    };
    let data = format!("{value}\n");
    let destination = spool.join(format!("{key}.status"));
    if std::fs::read(&destination).is_ok_and(|d| d == data.as_bytes()) {
        return Ok(());
    }
    crate::fsx::atomic_write(&destination, data.as_bytes())
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The delivery journal of one spool.
pub struct Relay<S: Service> {
    spool: PathBuf,
    service: S,
    db: Connection,
    clock: Box<dyn Fn() -> f64 + Send>,
}

impl<S: Service> Relay<S> {
    pub fn open(spool: &Path, service: S) -> Result<Relay<S>> {
        Self::with_clock(spool, service, Box::new(now))
    }

    pub fn with_clock(spool: &Path, service: S, clock: Box<dyn Fn() -> f64 + Send>) -> Result<Relay<S>> {
        let db = Connection::open(spool.join("relay.sqlite3")).map_err(sql)?;
        db.execute_batch(
            "PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS deliveries (
                key TEXT PRIMARY KEY, digest TEXT NOT NULL, marker TEXT NOT NULL, state TEXT NOT NULL,
                number INTEGER, attempted REAL NOT NULL DEFAULT 0, next_try REAL NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value REAL NOT NULL);
             UPDATE deliveries SET state='uncertain' WHERE state='posting';",
        )
        .map_err(sql)?;
        Ok(Relay { spool: spool.into(), service, db, clock })
    }

    fn set_state(&self, key: &str, state: &str, number: Option<u64>, delay: u64) -> Result<()> {
        self.db
            .execute(
                "UPDATE deliveries SET state=?1, number=?2, next_try=?3 WHERE key=?4",
                params![state, number.map(|n| n as i64), (self.clock)() + delay as f64, key],
            )
            .map_err(sql)?;
        write_status(&self.spool, key, state, number)
    }

    fn set_network_gate(&self, delay: u64) -> Result<()> {
        self.db
            .execute("INSERT OR REPLACE INTO settings VALUES('next_network',?1)", params![(self.clock)() + delay as f64])
            .map_err(sql)?;
        Ok(())
    }

    /// Handle one report file; returns its delivery state.
    pub fn process(&self, path: &Path) -> Result<String> {
        let Some(key) = path.file_stem().and_then(|s| s.to_str()).filter(|k| valid_key(k)) else {
            return Ok("ignored".into());
        };
        let report = match read_report(path) {
            Ok(r) => r,
            Err(_) => {
                write_status(&self.spool, key, "failed", None)?;
                return Ok("failed".into());
            }
        };
        self.db
            .execute(
                "INSERT OR IGNORE INTO deliveries(key,digest,marker,state) VALUES(?1,?2,?3,'queued')",
                params![key, report.digest, report.marker],
            )
            .map_err(sql)?;
        let (digest, row_state, number, next_try): (String, String, Option<i64>, f64) = self
            .db
            .query_row("SELECT digest, state, number, next_try FROM deliveries WHERE key=?1", params![key], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(sql)?;
        let number = number.map(|n| n as u64);
        if digest != report.digest {
            // Never replace an existing delivery or publish edited content under the same ID.
            write_status(&self.spool, key, "failed", None)?;
            return Ok("changed".into());
        }
        if matches!(row_state.as_str(), "created" | "failed" | "uncertain") {
            write_status(&self.spool, key, &row_state, number)?;
            return Ok(row_state);
        }
        if next_try > (self.clock)() {
            write_status(&self.spool, key, &row_state, None)?;
            return Ok(row_state);
        }
        let gate: Option<f64> = self
            .db
            .query_row("SELECT value FROM settings WHERE key='next_network'", [], |r| r.get(0))
            .optional()
            .map_err(sql)?;
        if gate.is_some_and(|g| g > (self.clock)()) {
            write_status(&self.spool, key, &row_state, None)?;
            return Ok(row_state);
        }
        self.set_network_gate(5)?;
        // A crash after this point needs review: the service's duplicate cache can reset on deploy.
        self.db
            .execute("UPDATE deliveries SET state='posting', attempted=?1 WHERE key=?2", params![(self.clock)(), key])
            .map_err(sql)?;
        match self.service.create(&report.title, &report.body) {
            Ok(n) => {
                self.set_state(key, "created", Some(n), 0)?;
                Ok("created".into())
            }
            Err(failure) => {
                let (state, delay) = match failure {
                    Failure::Limited(d) | Failure::Pending(d) => ("queued", d),
                    Failure::Blocked(d) => ("blocked", d),
                    Failure::Invalid => ("failed", 300),
                    Failure::Uncertain => ("uncertain", 300),
                };
                self.set_state(key, state, None, delay)?;
                if let Failure::Limited(d) | Failure::Blocked(d) = failure {
                    self.set_network_gate(d)?;
                }
                Ok(state.into())
            }
        }
    }

    /// One pass over the spool, in name order.
    pub fn pass(&self) -> Result<()> {
        let mut reports: Vec<PathBuf> = std::fs::read_dir(&self.spool)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "report"))
            .collect();
        reports.sort();
        for path in reports {
            self.process(&path)?;
        }
        Ok(())
    }
}

fn sql(e: rusqlite::Error) -> Error {
    Error::Invalid(format!("The bug-report journal failed ({e})."))
}

/// `relay-worker`: deliver reports until the supervisor asks the relay to stop (`.state/stop-relay`).
pub fn worker(root: &Path) -> Result<()> {
    let spool = root.join("BugReport/reports");
    std::fs::create_dir_all(&spool)?;
    let _lock = state::FileLock::try_acquire(&spool.join("relay.lock"))?
        .ok_or_else(|| Error::Invalid("Another bug-report relay is already using this spool.".into()))?;
    let key = std::env::var("COA_BUGREPORT_API_KEY").unwrap_or_else(|_| DEFAULT_API_KEY.into());
    let relay = Relay::open(&spool, ReportService::new(SERVICE_URL, &key)?)?;
    state::record(root, "relay-ready", std::process::id())?;
    println!("Bug-report relay ready. Waiting for in-game reports.");
    let stop = state::dir(root).join("stop-relay");
    while !stop.exists() {
        relay.pass()?;
        for _ in 0..10 {
            if stop.exists() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Answers = Arc<Mutex<Vec<std::result::Result<u64, Failure>>>>;

    struct Fake(Answers, Arc<Mutex<u32>>);
    impl Service for Fake {
        fn create(&self, _: &str, _: &str) -> std::result::Result<u64, Failure> {
            *self.1.lock().unwrap() += 1;
            self.0.lock().unwrap().remove(0)
        }
    }

    fn report(spool: &Path, key: &str, text: &str) -> PathBuf {
        let p = spool.join(format!("{key}.report"));
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn keys_follow_the_world_format() {
        assert!(valid_key("1-0123456789abcdef"));
        assert!(valid_key("4294967295-0123456789abcdef0123456789abcdef0123456789abcdef"));
        for bad in ["0-0123456789abcdef", "1-0123456789abcde", "1-0123456789ABCDEF", "12345678901-0123456789abcdef", "x"] {
            assert!(!valid_key(bad), "{bad}");
        }
    }

    #[test]
    fn service_answers_map_like_relay_py() {
        let ok = format!(r#"{{"status":"created","issue_number":12,"issue_url":"https://github.com/{REPOSITORY}/issues/12"}}"#);
        let cases: Vec<(u16, Option<&str>, Option<&[u8]>, std::result::Result<u64, Failure>)> = vec![
            (201, None, Some(ok.as_bytes()), Ok(12)),
            (200, None, Some(ok.as_bytes()), Ok(12)),
            (201, None, Some(br#"{"status":"created","issue_number":12,"issue_url":"https://evil/12"}"#), Err(Failure::Uncertain)),
            (201, None, Some(br#"{"status":"created","issue_number":0,"issue_url":"x"}"#), Err(Failure::Uncertain)),
            (201, None, Some(b"[1]"), Err(Failure::Uncertain)),
            (201, None, None, Err(Failure::Uncertain)),
            (202, Some("7"), Some(br#"{"status":"pending"}"#), Err(Failure::Pending(7))),
            (202, None, Some(br#"{"status":"pending"}"#), Err(Failure::Pending(2))),
            (202, None, Some(br#"{"status":"other"}"#), Err(Failure::Uncertain)),
            (429, Some("999999"), None, Err(Failure::Limited(86400))),
            (429, Some("0"), None, Err(Failure::Limited(1))),
            (429, Some("soon"), None, Err(Failure::Limited(300))),
            (422, None, None, Err(Failure::Invalid)),
            (302, None, None, Err(Failure::Blocked(300))),
            (401, None, None, Err(Failure::Blocked(300))),
            (503, Some("30"), Some(br#"{"status":"failed","error":"github_access_denied"}"#), Err(Failure::Blocked(30))),
            (503, None, Some(b"{}"), Err(Failure::Uncertain)),
            (500, None, None, Err(Failure::Uncertain)),
        ];
        for (i, (status, retry, body, expected)) in cases.into_iter().enumerate() {
            assert_eq!(classify(status, retry, body), expected, "case {i}");
        }
        assert!(json_object(Some(&vec![b' '; MAX_RESPONSE + 1])).is_none(), "oversized answers are refused");
        assert!(ReportService::new(SERVICE_URL, "has space").is_err());
        assert!(ReportService::new(SERVICE_URL, "coa_key").is_ok());
    }

    #[test]
    fn a_report_is_checked_and_identified_like_relay_py() {
        let d = tempfile::tempdir().unwrap();
        let p = report(d.path(), "7-0123456789abcdef", "COABUG1\r\nBroken quest\r\nThe NPC is missing.\r\n");
        let r = read_report(&p).unwrap();
        assert_eq!(r.title, "Broken quest");
        assert_eq!(r.body, "The NPC is missing.\n");
        assert_eq!(r.digest, hex(&Sha256::digest(std::fs::read(&p).unwrap())));
        assert!(r.marker.starts_with("<!-- coa-report:") && r.marker.len() == 16 + 64 + 4);
        for bad in ["COABUG2\nTitle\nBody", "COABUG1\nab\nBody", "COABUG1\nTitle\n  \n", "COABUG1\nTi\u{7}tle\nBody", "COABUG1\nTitle"] {
            std::fs::write(&p, bad).unwrap();
            assert!(read_report(&p).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn deliveries_follow_the_journal() {
        let d = tempfile::tempdir().unwrap();
        let spool = d.path();
        let t = Arc::new(Mutex::new(1000.0f64));
        let calls = Arc::new(Mutex::new(0));
        let answers: Answers = Arc::new(Mutex::new(vec![Err(Failure::Limited(60)), Ok(42)]));
        let clock = t.clone();
        let relay = Relay::with_clock(spool, Fake(answers, calls.clone()), Box::new(move || *clock.lock().unwrap())).unwrap();
        let p = report(spool, "7-0123456789abcdef", "COABUG1\nBroken quest\nThe NPC is missing.\n");
        let status = || std::fs::read_to_string(spool.join("7-0123456789abcdef.status")).unwrap();
        assert_eq!(relay.process(&p).unwrap(), "queued", "429 keeps it queued");
        assert_eq!(status(), "queued\n");
        assert_eq!(relay.process(&p).unwrap(), "queued", "not before its retry time");
        assert_eq!(*calls.lock().unwrap(), 1);
        *t.lock().unwrap() += 61.0;
        assert_eq!(relay.process(&p).unwrap(), "created");
        assert_eq!(status(), "created|42\n");
        assert_eq!(relay.process(&p).unwrap(), "created", "never sent twice");
        assert_eq!(*calls.lock().unwrap(), 2);
        std::fs::write(&p, "COABUG1\nBroken quest\nEdited.\n").unwrap();
        assert_eq!(relay.process(&p).unwrap(), "changed");
        relay
            .db
            .execute("INSERT INTO deliveries(key,digest,marker,state) VALUES('8-0123456789abcdef','x','m','posting')", [])
            .unwrap();
        drop(relay);
        let reopened = Relay::open(spool, Fake(Arc::new(Mutex::new(vec![])), calls.clone())).unwrap();
        let state: String =
            reopened.db.query_row("SELECT state FROM deliveries WHERE key='8-0123456789abcdef'", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "uncertain", "an interrupted post is never retried");
    }

    #[test]
    fn a_second_worker_cannot_take_the_spool() {
        let d = tempfile::tempdir().unwrap();
        let first = state::FileLock::try_acquire(&d.path().join("relay.lock")).unwrap();
        assert!(first.is_some());
        assert!(state::FileLock::try_acquire(&d.path().join("relay.lock")).unwrap().is_none());
        drop(first);
        assert!(state::FileLock::try_acquire(&d.path().join("relay.lock")).unwrap().is_some());
    }
}
