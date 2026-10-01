//! Resumable, verified downloads. Data goes to `<dest>.part`, is hashed as a whole, and is renamed into place
//! only after the SHA-256 matches. Nothing downloaded is ever executed or extracted before that.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::fsx;

#[derive(Debug, Clone)]
pub struct Job {
    pub url: String,
    pub dest: PathBuf,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    pub bytes_per_sec: u64,
}

#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// HTTPS only. Plain HTTP is accepted solely for loopback (a locally hosted package).
pub fn check_url(url: &str) -> Result<()> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        if matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Ok(());
        }
    }
    Err(Error::Invalid("Downloads must use HTTPS.".into()))
}

/// One HTTP response as far as the downloader cares.
pub struct Reply {
    pub status: u16,
    pub content_range: Option<String>,
    pub body: Box<dyn Read + Send>,
}

/// Network access behind a trait so resume/retry logic is testable without sockets.
pub trait Transport {
    fn get(&self, url: &str, range_from: u64) -> std::result::Result<Reply, String>;
}

pub struct HttpTransport(reqwest::blocking::Client);

impl HttpTransport {
    /// For small documents (manifests): gives up when the server stalls instead of waiting forever.
    pub fn with_total_timeout(secs: u64) -> Result<Self> {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(secs))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map(HttpTransport)
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    pub fn new() -> Result<Self> {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(None::<Duration>)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map(HttpTransport)
            .map_err(|e| Error::Invalid(e.to_string()))
    }
}

impl Transport for HttpTransport {
    fn get(&self, url: &str, range_from: u64) -> std::result::Result<Reply, String> {
        let mut req = self.0.get(url);
        if range_from > 0 {
            req = req.header("Range", format!("bytes={range_from}-"));
        }
        let resp = req.send().map_err(|e| e.to_string())?;
        Ok(Reply {
            status: resp.status().as_u16(),
            content_range: resp.headers().get("content-range").and_then(|v| v.to_str().ok()).map(str::to_string),
            body: Box::new(resp),
        })
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

fn hash_prefix(path: &Path, hasher: &mut Sha256) -> std::io::Result<u64> {
    let mut f = File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
}

enum Attempt {
    Done,
    Retry(String),
}

fn attempt(client: &dyn Transport, job: &Job, part: &Path, cancel: &Cancel, on_progress: &dyn Fn(Progress)) -> Result<Attempt> {
    let mut hasher = Sha256::new();
    let mut have = if part.exists() { hash_prefix(part, &mut hasher)? } else { 0 };
    if have > job.size {
        fs::remove_file(part)?;
        hasher = Sha256::new();
        have = 0;
    }
    if have == job.size {
        if !part.exists() {
            // an empty file: nothing to request
            File::create(part)?;
        }
        return finish(job, part, hasher);
    }

    let mut resp = match client.get(&job.url, have) {
        Ok(r) => r,
        Err(e) => return Ok(Attempt::Retry(e)),
    };
    if have > 0 && resp.status == 200 {
        // Server ignored the Range header: start over rather than corrupt the file.
        drop(resp);
        fs::remove_file(part)?;
        return Ok(Attempt::Retry("server does not support resuming; restarting".into()));
    }
    if !(resp.status == 200 || resp.status == 206) {
        return Err(Error::Invalid(format!("The download server answered {}.", resp.status)));
    }
    if resp.status == 206 {
        let ok = resp.content_range.as_deref().map(|v| v.starts_with(&format!("bytes {have}-"))).unwrap_or(false);
        if !ok {
            fs::remove_file(part)?;
            return Ok(Attempt::Retry("unexpected byte range; restarting".into()));
        }
    }

    let mut file = OpenOptions::new().create(true).append(true).open(part)?;
    let started = Instant::now();
    let base = have;
    let mut buf = vec![0u8; 256 * 1024];
    let mut last_report = Instant::now();
    loop {
        if cancel.is_set() {
            return Err(Error::Invalid("Download cancelled.".into()));
        }
        let n = match resp.body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Ok(Attempt::Retry(e.to_string())),
        };
        if have + n as u64 > job.size {
            drop(file);
            fs::remove_file(part)?;
            return Err(Error::HashMismatch {
                path: job.dest.display().to_string(),
                expected: format!("{} bytes", job.size),
                actual: "more data than expected".into(),
            });
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        have += n as u64;
        if last_report.elapsed() >= Duration::from_millis(250) {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            on_progress(Progress { downloaded: have, total: job.size, bytes_per_sec: ((have - base) as f64 / secs) as u64 });
            last_report = Instant::now();
        }
    }
    file.flush()?;
    drop(file);
    if have < job.size {
        return Ok(Attempt::Retry(format!("connection ended after {have} of {} bytes", job.size)));
    }
    on_progress(Progress { downloaded: have, total: job.size, bytes_per_sec: 0 });
    finish(job, part, hasher)
}

fn finish(job: &Job, part: &Path, hasher: Sha256) -> Result<Attempt> {
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(&job.sha256) {
        let _ = fs::remove_file(part);
        return Err(Error::HashMismatch { path: job.dest.display().to_string(), expected: job.sha256.clone(), actual });
    }
    fs::rename(part, &job.dest)?;
    Ok(Attempt::Done)
}

/// Download `job` (resuming a previous `.part`), retrying network failures with backoff.
pub fn fetch(job: &Job, cancel: &Cancel, on_progress: &dyn Fn(Progress)) -> Result<()> {
    check_url(&job.url)?;
    fetch_with(&HttpTransport::new()?, job, cancel, on_progress)
}

pub fn fetch_with(client: &dyn Transport, job: &Job, cancel: &Cancel, on_progress: &dyn Fn(Progress)) -> Result<()> {
    if let Some(parent) = job.dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if job.dest.is_file() && fsx::sha256_file(&job.dest).map(|h| h.eq_ignore_ascii_case(&job.sha256)).unwrap_or(false) {
        on_progress(Progress { downloaded: job.size, total: job.size, bytes_per_sec: 0 });
        return Ok(());
    }
    let part = part_path(&job.dest);
    fsx::require_space(&job.dest, job.size.saturating_sub(part.metadata().map(|m| m.len()).unwrap_or(0)))?;
    let mut last_err = String::new();
    for n in 0..6u32 {
        if cancel.is_set() {
            return Err(Error::Invalid("Download cancelled.".into()));
        }
        match attempt(client, job, &part, cancel, on_progress)? {
            Attempt::Done => return Ok(()),
            Attempt::Retry(why) => {
                tracing::warn!(url = %job.url, attempt = n + 1, %why, "download interrupted; will resume");
                last_err = why;
                std::thread::sleep(Duration::from_millis(if cfg!(test) { 1 } else { 300 * 2u64.pow(n.min(4)) }));
            }
        }
    }
    Err(Error::Invalid(format!("The download kept failing ({last_err}). Check your connection and try again; progress is kept.")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::AtomicUsize;

    /// In-memory server: optionally cuts the first `cut_first` bodies in half and optionally ignores Range.
    struct Mock {
        body: Vec<u8>,
        cut_first: usize,
        support_range: bool,
        calls: AtomicUsize,
        ranges: std::sync::Mutex<Vec<u64>>,
    }

    struct Cut(Cursor<Vec<u8>>, usize);

    impl Read for Cut {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0.position() as usize >= self.1 {
                return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection lost"));
            }
            let lim = (self.1 - self.0.position() as usize).min(buf.len());
            self.0.read(&mut buf[..lim])
        }
    }

    impl Mock {
        fn new(body: Vec<u8>, cut_first: usize, support_range: bool) -> Self {
            Mock { body, cut_first, support_range, calls: AtomicUsize::new(0), ranges: Default::default() }
        }
    }

    impl Transport for Mock {
        fn get(&self, _url: &str, from: u64) -> std::result::Result<Reply, String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            self.ranges.lock().unwrap().push(from);
            let (status, start, cr) = if from > 0 && self.support_range {
                (206, from as usize, Some(format!("bytes {from}-{}/{}", self.body.len() - 1, self.body.len())))
            } else {
                (200, 0, None)
            };
            let slice = self.body[start..].to_vec();
            let cut = if n < self.cut_first { slice.len() / 2 } else { slice.len() };
            Ok(Reply { status, content_range: cr, body: Box::new(Cut(Cursor::new(slice), cut)) })
        }
    }

    fn body() -> Vec<u8> {
        (0..600_000u32).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
    }

    fn job(dir: &Path, body: &[u8]) -> Job {
        Job { url: "https://example.invalid/pkg".into(), dest: dir.join("pkg.bin"), sha256: fsx::sha256_bytes(body), size: body.len() as u64 }
    }

    #[test]
    fn resumes_after_connection_drops_at_half_and_verifies() {
        let body = body();
        let dir = tempfile::tempdir().unwrap();
        let m = Mock::new(body.clone(), 1, true);
        let j = job(dir.path(), &body);
        fetch_with(&m, &j, &Cancel::default(), &|_| {}).unwrap();
        assert_eq!(fs::read(&j.dest).unwrap(), body);
        assert_eq!(m.ranges.lock().unwrap().clone(), vec![0, 300_000], "second request resumed exactly where the first stopped");
        assert!(!part_path(&j.dest).exists());
    }

    #[test]
    fn restarts_cleanly_when_server_cannot_resume() {
        let body = body();
        let dir = tempfile::tempdir().unwrap();
        let m = Mock::new(body.clone(), 1, false);
        let j = job(dir.path(), &body);
        fetch_with(&m, &j, &Cancel::default(), &|_| {}).unwrap();
        assert_eq!(fs::read(&j.dest).unwrap(), body);
    }

    #[test]
    fn a_leftover_part_file_from_a_previous_run_is_resumed_and_hashed_as_a_whole() {
        let body = body();
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path(), &body);
        fs::write(part_path(&j.dest), &body[..123_456]).unwrap();
        let m = Mock::new(body.clone(), 0, true);
        fetch_with(&m, &j, &Cancel::default(), &|_| {}).unwrap();
        assert_eq!(m.ranges.lock().unwrap().clone(), vec![123_456]);
        assert_eq!(fs::read(&j.dest).unwrap(), body);
    }

    #[test]
    fn corrupt_leftover_never_reaches_destination() {
        let body = body();
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path(), &body);
        fs::write(part_path(&j.dest), vec![7u8; 1000]).unwrap();
        let m = Mock::new(body.clone(), 0, true);
        assert!(matches!(fetch_with(&m, &j, &Cancel::default(), &|_| {}), Err(Error::HashMismatch { .. })));
        assert!(!j.dest.exists() && !part_path(&j.dest).exists());
    }

    #[test]
    fn wrong_hash_and_oversized_bodies_are_rejected() {
        let body = body();
        let dir = tempfile::tempdir().unwrap();
        let mut j = job(dir.path(), &body);
        j.sha256 = "0".repeat(64);
        assert!(matches!(fetch_with(&Mock::new(body.clone(), 0, true), &j, &Cancel::default(), &|_| {}), Err(Error::HashMismatch { .. })));
        assert!(!j.dest.exists());
        let mut small = job(dir.path(), &body);
        small.size = 1000;
        assert!(fetch_with(&Mock::new(body.clone(), 0, true), &small, &Cancel::default(), &|_| {}).is_err());
    }

    #[test]
    fn already_downloaded_file_is_reused_and_cancel_keeps_progress() {
        let body = body();
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path(), &body);
        fs::write(&j.dest, &body).unwrap();
        let m = Mock::new(body.clone(), 0, true);
        fetch_with(&m, &j, &Cancel::default(), &|_| {}).unwrap();
        assert_eq!(m.calls.load(Ordering::SeqCst), 0, "no network use when the file is already verified");
        fs::remove_file(&j.dest).unwrap();
        let c = Cancel::default();
        c.cancel();
        assert!(fetch_with(&m, &j, &c, &|_| {}).is_err());
    }

    #[test]
    fn gives_up_after_repeated_failures_with_a_friendly_message() {
        let body = body();
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path(), &body);
        let m = Mock::new(body.clone(), usize::MAX, true);
        let e = fetch_with(&m, &j, &Cancel::default(), &|_| {}).unwrap_err();
        assert!(e.to_string().contains("progress is kept"));
        assert!(part_path(&j.dest).exists(), "partial data is kept for the next try");
    }

    #[test]
    fn an_empty_file_needs_no_request_and_is_still_created() {
        let dir = tempfile::tempdir().unwrap();
        let j = job(dir.path(), b"");
        let m = Mock::new(Vec::new(), 0, true);
        fetch_with(&m, &j, &Cancel::default(), &|_| {}).unwrap();
        assert_eq!(fs::read(&j.dest).unwrap(), Vec::<u8>::new());
        assert_eq!(m.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn only_https_or_loopback_http_is_allowed() {
        assert!(check_url("https://github.com/x").is_ok());
        assert!(check_url("http://127.0.0.1:8000/x").is_ok());
        assert!(check_url("http://localhost/x").is_ok());
        assert!(check_url("http://example.com/x").is_err());
        assert!(check_url("ftp://example.com/x").is_err());
        assert!(check_url("file:///c:/x").is_err());
    }
}
