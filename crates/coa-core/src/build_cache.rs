//! The compiler's work kept between builds of a module, small on disk.
//!
//! A build works in `<meta>/build-cache/<module>/` (sources, CMake/Ninja state, object files). Between builds
//! that folder is packed into `<module>.tar.zst` (object files compress about 4x) with a small record of the
//! core it was made for. Before the next build it is unpacked again, so Ninja recompiles only what changed.
//!
//! Object files made for another core are useless (every one depends on the core's headers), so a cache for
//! another core is dropped instead of unpacked. The whole cache is kept under a size limit: the modules built
//! longest ago go first. Incremental-link files (`.ilk`) and linker maps are not kept.

use std::fs;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Packed caches above this total are removed, the least recently built first.
pub const LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    /// ABI fingerprint of the core the objects were compiled for.
    fingerprint: String,
    /// Unix seconds of the last build.
    last_used: i64,
    unpacked_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheEntry {
    pub module: String,
    /// Bytes on disk now (packed, or unpacked while a build runs).
    pub bytes: u64,
    pub unpacked_bytes: u64,
    pub last_used: i64,
    /// Made for the core this server runs now (otherwise it is dropped at the next build).
    pub current_core: bool,
    pub packed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheSummary {
    pub modules: Vec<CacheEntry>,
    pub total_bytes: u64,
    /// Downloaded module sources (shared by all servers of this Manager).
    pub sources_bytes: u64,
    pub limit_bytes: u64,
}

pub fn root(realm_root: &Path) -> PathBuf {
    crate::plugins::meta_dir(realm_root).join("build-cache")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && id != "." && id != ".."
}

fn paths(realm_root: &Path, id: &str) -> Result<(PathBuf, PathBuf, PathBuf)> {
    if !valid_id(id) {
        return Err(Error::Invalid(format!("Invalid module id for the build cache: {id}")));
    }
    let r = root(realm_root);
    Ok((r.join(id), r.join(format!("{id}.tar.zst")), r.join(format!("{id}.json"))))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(crate) fn dir_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => dir_bytes(&e.path()),
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

fn read_record(path: &Path) -> Option<Record> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// The working folder for a build of `id` for the core `fingerprint`, with the earlier work of a build for the
/// same core unpacked into it. A cache for another core is dropped.
pub fn prepare(realm_root: &Path, id: &str, fingerprint: &str) -> Result<PathBuf> {
    let (dir, archive, record) = paths(realm_root, id)?;
    let rec = read_record(&record);
    let same_core = rec.as_ref().is_some_and(|r| r.fingerprint == fingerprint);
    if !same_core {
        // made for another core (or unknown): its objects would all be rebuilt anyway
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        let _ = fs::remove_file(&archive);
        let _ = fs::remove_file(&record);
    } else if archive.is_file() {
        if dir.exists() {
            fs::remove_dir_all(&dir)?; // an older, interrupted working folder: the archive is the newer state
        }
        unpack(&archive, &dir).map_err(|e| {
            let _ = fs::remove_dir_all(&dir);
            Error::Invalid(format!("The build cache of {id} could not be unpacked: {e}"))
        })?;
        fs::remove_file(&archive)?;
    }
    fs::create_dir_all(&dir)?;
    let unpacked_bytes = dir_bytes(&dir);
    crate::fsx::atomic_write_json(&record, &Record { fingerprint: fingerprint.into(), last_used: now(), unpacked_bytes })?;
    Ok(dir)
}

/// After a build (successful or not): drop what does not speed up the next one, pack the rest, keep the total
/// under the limit. Never fails the build: problems only cost the cache.
pub fn finish(realm_root: &Path, id: &str) {
    let Ok((dir, archive, record)) = paths(realm_root, id) else { return };
    if !dir.is_dir() {
        return;
    }
    remove_by_ext(&dir, &["ilk", "map"]);
    let unpacked_bytes = dir_bytes(&dir);
    let tmp = archive.with_extension("zst.partial");
    match pack(&dir, &tmp).and_then(|_| fs::rename(&tmp, &archive).map_err(Into::into)) {
        Ok(()) => {
            let _ = fs::remove_dir_all(&dir);
            if let Some(mut r) = read_record(&record) {
                r.last_used = now();
                r.unpacked_bytes = unpacked_bytes;
                let _ = crate::fsx::atomic_write_json(&record, &r);
            }
        }
        Err(e) => {
            tracing::warn!(module = id, error = %e, "build cache not packed; it is kept unpacked");
            let _ = fs::remove_file(&tmp);
        }
    }
    enforce_limit(realm_root, LIMIT_BYTES, Some(id));
}

fn remove_by_ext(dir: &Path, exts: &[&str]) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_by_ext(&p, exts);
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.iter().any(|w| w.eq_ignore_ascii_case(x))) {
            let _ = fs::remove_file(p);
        }
    }
}

fn pack(dir: &Path, out: &Path) -> Result<()> {
    let file = BufWriter::new(fs::File::create(out)?);
    let mut enc = zstd::Encoder::new(file, 3)?;
    let _ = enc.multithread(std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1).min(8));
    {
        let mut tar = tar::Builder::new(&mut enc);
        tar.follow_symlinks(false);
        // modification times are kept: Ninja decides from them what is up to date
        tar.mode(tar::HeaderMode::Complete);
        tar.append_dir_all(".", dir)?;
        tar.finish()?;
    }
    enc.finish()?;
    Ok(())
}

fn unpack(archive: &Path, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let dec = zstd::Decoder::new(BufReader::new(fs::File::open(archive)?))?;
    let mut tar = tar::Archive::new(dec);
    tar.set_preserve_mtime(true);
    for entry in tar.entries()? {
        let mut entry = entry?;
        // unpack_in refuses paths that would leave `dir`
        if !entry.unpack_in(dir)? {
            return Err(Error::PathRejected(format!("build cache entry {}", entry.path()?.display())));
        }
    }
    Ok(())
}

/// Everything in the build cache of this server, largest first.
pub fn summary(realm_root: &Path, sources_dir: Option<&Path>) -> CacheSummary {
    let current = crate::provenance::realm_host_fingerprint(realm_root);
    let r = root(realm_root);
    let mut modules = Vec::new();
    for e in fs::read_dir(&r).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json") else { continue };
        let Ok((dir, archive, record)) = paths(realm_root, id) else { continue };
        let Some(rec) = read_record(&record) else { continue };
        let packed = archive.is_file();
        let bytes = if packed { fs::metadata(&archive).map(|m| m.len()).unwrap_or(0) } else { dir_bytes(&dir) };
        modules.push(CacheEntry {
            module: id.to_string(),
            bytes,
            unpacked_bytes: rec.unpacked_bytes,
            last_used: rec.last_used,
            current_core: current.as_deref() == Some(rec.fingerprint.as_str()),
            packed,
        });
    }
    // working folders of an older Manager, without a record
    for e in fs::read_dir(&r).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() && valid_id(&name) && !modules.iter().any(|m| m.module == name) {
            let bytes = dir_bytes(&e.path());
            modules.push(CacheEntry { module: name, bytes, unpacked_bytes: bytes, last_used: 0, current_core: false, packed: false });
        }
    }
    modules.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    CacheSummary {
        total_bytes: modules.iter().map(|m| m.bytes).sum(),
        sources_bytes: sources_dir.map(dir_bytes).unwrap_or(0),
        limit_bytes: LIMIT_BYTES,
        modules,
    }
}

/// Remove the cache of one module, or of all. The next build of a module compiles it in full.
pub fn clear(realm_root: &Path, module: Option<&str>) -> Result<u64> {
    let before = summary(realm_root, None).total_bytes;
    match module {
        Some(id) => {
            let (dir, archive, record) = paths(realm_root, id)?;
            if dir.exists() {
                fs::remove_dir_all(dir)?;
            }
            let _ = fs::remove_file(archive);
            let _ = fs::remove_file(record);
        }
        None => {
            let r = root(realm_root);
            if r.exists() {
                fs::remove_dir_all(&r)?;
            }
        }
    }
    Ok(before.saturating_sub(summary(realm_root, None).total_bytes))
}

/// Keep the packed caches under `limit`, dropping the least recently built (never `keep`).
fn enforce_limit(realm_root: &Path, limit: u64, keep: Option<&str>) {
    let mut s = summary(realm_root, None);
    s.modules.sort_by_key(|m| m.last_used);
    let mut total = s.total_bytes;
    for m in &s.modules {
        if total <= limit {
            break;
        }
        if Some(m.module.as_str()) == keep {
            continue;
        }
        if clear(realm_root, Some(&m.module)).is_ok() {
            tracing::info!(module = %m.module, bytes = m.bytes, "build cache over its limit: oldest removed");
            total = total.saturating_sub(m.bytes);
        }
    }
}

/// Downloaded module sources the Manager keeps (`<cache>/github`, `<cache>/staging`).
pub fn clear_sources(sources_dir: &Path) -> Result<u64> {
    let before = dir_bytes(sources_dir);
    for sub in ["github", "staging"] {
        let p = sources_dir.join(sub);
        if p.exists() {
            fs::remove_dir_all(p)?;
        }
    }
    Ok(before.saturating_sub(dir_bytes(sources_dir)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn server() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join(".manager")).unwrap();
        d
    }

    #[test]
    fn a_cache_is_packed_between_builds_and_comes_back_with_its_times() {
        let s = server();
        let dir = prepare(s.path(), "companions", "fp1").unwrap();
        fs::create_dir_all(dir.join("build/obj")).unwrap();
        fs::write(dir.join("build/obj/a.obj"), vec![7u8; 100_000]).unwrap();
        fs::write(dir.join("build/x.ilk"), b"incremental link").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        fs::File::options().write(true).open(dir.join("build/obj/a.obj")).unwrap().set_modified(old).unwrap();
        finish(s.path(), "companions");
        assert!(!dir.exists(), "packed between builds");
        let sum = summary(s.path(), None);
        assert_eq!(sum.modules.len(), 1);
        assert!(sum.modules[0].packed && sum.modules[0].bytes < 100_000 / 4, "compressed: {}", sum.modules[0].bytes);

        let dir = prepare(s.path(), "companions", "fp1").unwrap();
        assert_eq!(fs::read(dir.join("build/obj/a.obj")).unwrap().len(), 100_000);
        assert!(!dir.join("build/x.ilk").exists(), "incremental-link files are not kept");
        let mtime = fs::metadata(dir.join("build/obj/a.obj")).unwrap().modified().unwrap();
        let drift = mtime.duration_since(old).unwrap_or_else(|e| e.duration());
        assert!(drift < Duration::from_secs(2), "modification time kept (drift {drift:?})");
    }

    #[test]
    fn a_cache_for_another_core_is_dropped() {
        let s = server();
        let dir = prepare(s.path(), "scaling", "old-core").unwrap();
        fs::write(dir.join("a.obj"), b"old").unwrap();
        finish(s.path(), "scaling");
        let dir = prepare(s.path(), "scaling", "new-core").unwrap();
        assert!(!dir.join("a.obj").exists());
    }

    #[test]
    fn clear_removes_one_module_or_all_and_bad_ids_are_refused() {
        let s = server();
        for id in ["a", "b"] {
            let d = prepare(s.path(), id, "fp").unwrap();
            fs::write(d.join("x.obj"), b"x").unwrap();
            finish(s.path(), id);
        }
        clear(s.path(), Some("a")).unwrap();
        assert_eq!(summary(s.path(), None).modules.iter().map(|m| m.module.as_str()).collect::<Vec<_>>(), ["b"]);
        clear(s.path(), None).unwrap();
        assert!(summary(s.path(), None).modules.is_empty());
        assert!(clear(s.path(), Some("../x")).is_err());
    }

    #[test]
    fn the_least_recently_built_go_first_over_the_limit() {
        let s = server();
        for (n, id) in ["old", "new"].iter().enumerate() {
            let d = prepare(s.path(), id, "fp").unwrap();
            fs::write(d.join("x.obj"), vec![n as u8; 50_000]).unwrap();
            finish(s.path(), id);
            let rec = root(s.path()).join(format!("{id}.json"));
            let mut r = read_record(&rec).unwrap();
            r.last_used = 1000 + n as i64;
            crate::fsx::atomic_write_json(&rec, &r).unwrap();
        }
        let total = summary(s.path(), None).total_bytes;
        enforce_limit(s.path(), total - 1, None);
        let left: Vec<String> = summary(s.path(), None).modules.into_iter().map(|m| m.module).collect();
        assert_eq!(left, ["new"]);
    }
}
