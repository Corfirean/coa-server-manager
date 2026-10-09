//! The only module allowed to mutate the filesystem. Every function validates its target first.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Resolve `.` and `..` lexically without touching the filesystem.
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn key(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| {
            let c = c.as_os_str().to_string_lossy();
            if cfg!(windows) {
                c.to_lowercase()
            } else {
                c.into_owned()
            }
        })
        .collect()
}

/// Component-wise prefix test: case-insensitive on Windows, exact elsewhere (`/srv/CoA` and `/srv/coa` are two folders).
pub fn starts_with_ci(path: &Path, prefix: &Path) -> bool {
    let (p, q) = (key(path), key(prefix));
    p.len() >= q.len() && p[..q.len()] == q[..]
}

/// Canonicalize the deepest existing ancestor, then re-append the not-yet-existing tail.
/// Detects symlink / junction escapes for paths that do not exist yet.
pub fn canonicalize_lenient(path: &Path) -> Result<PathBuf> {
    let path = normalize_lexical(path);
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path.as_path();
    loop {
        match dunce::canonicalize(cursor) {
            Ok(mut base) => {
                for part in tail.iter().rev() {
                    base.push(part);
                }
                return Ok(base);
            }
            Err(_) => match (cursor.file_name(), cursor.parent()) {
                (Some(name), Some(parent)) => {
                    tail.push(name.to_os_string());
                    cursor = parent;
                }
                _ => {
                    return Err(Error::PathRejected(format!(
                        "cannot resolve {}",
                        path.display()
                    )))
                }
            },
        }
    }
}

/// Validate that `candidate` (absolute, or relative to `root`) resolves inside `root`,
/// following existing symlinks/junctions. Returns the resolved path.
pub fn ensure_within(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let root = canonicalize_lenient(root)?;
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let resolved = canonicalize_lenient(&joined)?;
    if starts_with_ci(&resolved, &root) {
        Ok(resolved)
    } else {
        Err(Error::PathRejected(format!(
            "{} is outside {}",
            resolved.display(),
            root.display()
        )))
    }
}

/// Join an untrusted relative path (from a manifest / archive) onto `root`.
/// Rejects absolute paths, drive prefixes, `..`, empty components, NTFS streams and reserved device names.
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    let bad = |why: &str| Error::PathRejected(format!("{rel:?}: {why}"));
    if rel.is_empty() {
        return Err(bad("empty"));
    }
    if rel.contains(':') {
        return Err(bad("drive or stream marker"));
    }
    let normalized = rel.replace('\\', "/");
    if normalized.starts_with('/') {
        return Err(bad("absolute"));
    }
    let mut out = root.to_path_buf();
    for part in normalized.split('/') {
        match part {
            "" | "." => return Err(bad("empty or dot component")),
            ".." => return Err(bad("parent traversal")),
            p => {
                if p.ends_with('.') || p.ends_with(' ') {
                    return Err(bad("trailing dot or space"));
                }
                let stem = p.split('.').next().unwrap_or("").to_ascii_uppercase();
                const RESERVED: [&str; 22] = [
                    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
                    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
                    "LPT8", "LPT9",
                ];
                if RESERVED.contains(&stem.as_str()) {
                    return Err(bad("reserved device name"));
                }
                out.push(p);
            }
        }
    }
    Ok(out)
}

/// Write `bytes` to `path` so readers see either the old or the new content, never a partial file.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::PathRejected("no parent directory".into()))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        uuid::Uuid::new_v4().simple()
    ));
    let write = || -> std::io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    if let Err(e) = durable_replace(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(())
}

/// The temporary file must already be flushed, on the same volume as the destination.
pub(crate) fn durable_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        fn wide(path: &Path) -> std::io::Result<Vec<u16>> {
            let absolute = std::path::absolute(path)?;
            let text = absolute.as_os_str().to_string_lossy().replace('/', "\\");
            let extended = if text.starts_with("\\\\?\\") {
                text
            } else if let Some(unc) = text.strip_prefix("\\\\") {
                format!("\\\\?\\UNC\\{unc}")
            } else {
                format!("\\\\?\\{text}")
            };
            Ok(std::ffi::OsStr::new(&extended)
                .encode_wide()
                .chain(Some(0))
                .collect())
        }
        let src = wide(from)?;
        let dst = wide(to)?;
        if unsafe {
            MoveFileExW(
                src.as_ptr(),
                dst.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(from, to)?;
        File::open(to.parent().unwrap())?.sync_all()
    }
}

pub fn atomic_write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path)?;
    let bytes = bytes
        .strip_prefix(&[0xEF, 0xBB, 0xBF][..])
        .unwrap_or(&bytes);
    Ok(serde_json::from_slice(bytes)?)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Free bytes on the volume that holds `path` (or its nearest existing ancestor).
pub fn free_space(path: &Path) -> Result<u64> {
    let path = std::path::absolute(path)?;
    let mut cursor = Some(path.as_path());
    while let Some(p) = cursor {
        if p.exists() {
            return Ok(fs4::available_space(p)?);
        }
        cursor = p.parent();
    }
    Err(Error::PathRejected(format!(
        "no existing ancestor for {}",
        path.display()
    )))
}

/// Fail before writing if the volume cannot hold `needed` bytes plus a safety margin.
pub fn require_space(path: &Path, needed: u64) -> Result<()> {
    const MARGIN: u64 = 256 * 1024 * 1024;
    let available = free_space(path)?;
    if available < needed.saturating_add(MARGIN) {
        return Err(Error::InsufficientSpace {
            needed: needed.saturating_add(MARGIN),
            available,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_replacement_handles_long_transaction_backup_paths() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("a".repeat(90)).join("b".repeat(90));
        let path = parent.join("configuration-with-a-long-name.template");
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn safe_join_accepts_normal_and_rejects_traversal() {
        let root = Path::new("C:/srv");
        assert!(safe_join(root, "Core/worldserver.exe").is_ok());
        assert!(safe_join(root, "Core\\configs\\a.conf").is_ok());
        for bad in [
            "",
            "../x",
            "a/../../x",
            "/abs",
            "C:/x",
            "a/./b",
            "a//b",
            "a/CON",
            "a/nul.txt",
            "x.exe:zone",
            "a/b.",
            "a/b ",
        ] {
            assert!(safe_join(root, bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn ensure_within_blocks_escape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("srv");
        fs::create_dir_all(&root).unwrap();
        assert!(ensure_within(&root, Path::new("Core/new/file.txt")).is_ok());
        assert!(ensure_within(&root, Path::new("../evil")).is_err());
        assert!(ensure_within(&root, &dir.path().join("other")).is_err());
        // case-insensitive root match (Windows)
        let upper = PathBuf::from(root.to_string_lossy().to_uppercase()).join("a");
        if cfg!(windows) {
            assert!(ensure_within(&root, &upper).is_ok());
        }
    }

    #[test]
    fn ensure_within_blocks_junction_escape() {
        if !cfg!(windows) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("srv");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let link = root.join("link");
        // A directory junction needs no privileges (unlike symlinks).
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        if !status.status.success() {
            return;
        }
        assert!(ensure_within(&root, &link.join("file.txt")).is_err());
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), sha256_bytes(b"abc"));
    }

    #[test]
    fn read_json_tolerates_bom() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.json");
        fs::write(&p, b"\xEF\xBB\xBF{\"a\":1}").unwrap();
        let v: serde_json::Value = read_json(&p).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn require_space_rejects_absurd_request() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            require_space(dir.path(), u64::MAX / 2),
            Err(Error::InsufficientSpace { .. })
        ));
    }

    #[test]
    fn free_space_resolves_a_new_relative_directory() {
        let path = Path::new("coa-schema-fixture-not-created").join("nested");
        assert!(!path.exists());
        assert!(free_space(&path).unwrap() > 0);
    }
}
