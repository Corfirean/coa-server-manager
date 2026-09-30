//! Release manifest (`manifest.json`) shipped with every base/update/bots package.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;

pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Base,
    Update,
    Bots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Owner {
    Core,
    Bots,
    Manager,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReplacePolicy {
    Replace,
    ReplaceIfPristine,
    MergeConfig,
    CreateIfMissing,
    NeverTouch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub owner: Owner,
    pub policy: ReplacePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    pub id: String,
    pub db: String,
    pub sha256: String,
    #[serde(default)]
    pub destructive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivePart {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// How a base/update payload is shipped: one zstd-compressed tar stream cut into parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveInfo {
    pub format: String,
    pub parts: Vec<ArchivePart>,
    pub unpacked_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Revision {
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    pub kind: Kind,
    pub version: String,
    pub core: Revision,
    #[serde(default)]
    pub bots: Option<Revision>,
    pub built_at: String,
    pub min_manager_version: String,
    #[serde(default)]
    pub files: Vec<FileEntry>,
    #[serde(default)]
    pub migrations: Vec<Migration>,
    #[serde(default)]
    pub archive: Option<ArchiveInfo>,
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_commit(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse "1.2.3" leniently into a comparable tuple.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| Error::InvalidManifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |s: String| Err(Error::InvalidManifest(s));
        if self.schema != SCHEMA {
            return bad(format!("unsupported schema {}", self.schema));
        }
        if parse_version(&self.version).is_none() {
            return bad(format!("bad version {:?}", self.version));
        }
        if parse_version(&self.min_manager_version).is_none() {
            return bad(format!("bad minManagerVersion {:?}", self.min_manager_version));
        }
        for (label, rev) in [("core", Some(&self.core)), ("bots", self.bots.as_ref())] {
            if let Some(Revision { commit: Some(c) }) = rev {
                if !is_commit(c) {
                    return bad(format!("{label} commit must be a full 40-hex SHA, got {c:?}"));
                }
            }
        }
        let scratch = Path::new("root");
        let mut seen = HashSet::new();
        for f in &self.files {
            fsx::safe_join(scratch, &f.path).map_err(|e| Error::InvalidManifest(e.to_string()))?;
            if !is_sha256(&f.sha256) {
                return bad(format!("{}: bad sha256", f.path));
            }
            if !seen.insert(f.path.replace('\\', "/").to_lowercase()) {
                return bad(format!("{}: duplicate path", f.path));
            }
        }
        if let Some(a) = &self.archive {
            if a.format != "tar.zst" || a.parts.is_empty() {
                return bad(format!("unsupported archive format {:?}", a.format));
            }
            for p in &a.parts {
                if !is_sha256(&p.sha256) || p.name.contains(['/', '\\']) || p.name.is_empty() || p.name.starts_with('.') {
                    return bad(format!("archive part {:?} is invalid", p.name));
                }
            }
        }
        let mut ids = HashSet::new();
        for m in &self.migrations {
            if m.id.is_empty() || !is_sha256(&m.sha256) || !ids.insert((m.db.clone(), m.id.clone())) {
                return bad(format!("migration {:?} invalid or duplicated", m.id));
            }
        }
        Ok(())
    }

    /// True if this Manager build may apply the manifest.
    pub fn compatible_with_manager(&self, manager_version: &str) -> bool {
        match (parse_version(manager_version), parse_version(&self.min_manager_version)) {
            (Some(have), Some(need)) => have >= need,
            _ => false,
        }
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Verify `path` against the entry; never trusts size alone.
pub fn verify_file(path: &Path, entry: &FileEntry) -> Result<()> {
    let len = std::fs::metadata(path)?.len();
    if len != entry.size {
        return Err(Error::HashMismatch {
            path: entry.path.clone(),
            expected: format!("{} bytes", entry.size),
            actual: format!("{len} bytes"),
        });
    }
    let actual = fsx::sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(&entry.sha256) {
        return Err(Error::HashMismatch { path: entry.path.clone(), expected: entry.sha256.clone(), actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(files: &str) -> String {
        format!(
            r#"{{"schema":1,"kind":"update","version":"0.4.1","core":{{"commit":"{c}"}},"bots":{{"commit":null}},
            "builtAt":"2026-09-30T00:00:00Z","minManagerVersion":"0.1.0","files":[{files}],"migrations":[],"archive":null}}"#,
            c = "a".repeat(40)
        )
    }

    fn file(path: &str, sha: &str) -> String {
        format!(r#"{{"path":"{path}","sha256":"{sha}","size":3,"owner":"core","policy":"replace"}}"#)
    }

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(sample(&file("Core/worldserver.exe", &"b".repeat(64))).as_bytes()).unwrap();
        assert_eq!(m.files.len(), 1);
        assert!(m.compatible_with_manager("0.1.0"));
        assert!(!m.compatible_with_manager("0.0.9"));
    }

    #[test]
    fn rejects_traversal_bad_hash_duplicates_and_short_commit() {
        let ok = "b".repeat(64);
        assert!(Manifest::parse(sample(&file("../x", &ok)).as_bytes()).is_err());
        assert!(Manifest::parse(sample(&file("a/b", "zz")).as_bytes()).is_err());
        let dup = format!("{},{}", file("A/b", &ok), file("a/B", &ok));
        assert!(Manifest::parse(sample(&dup).as_bytes()).is_err());
        let s = sample("").replace(&"a".repeat(40), "master");
        assert!(Manifest::parse(s.as_bytes()).is_err());
    }

    #[test]
    fn verify_file_detects_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f");
        std::fs::write(&p, b"abc").unwrap();
        let entry = FileEntry {
            path: "f".into(),
            sha256: fsx::sha256_bytes(b"abc"),
            size: 3,
            owner: Owner::Core,
            policy: ReplacePolicy::Replace,
        };
        verify_file(&p, &entry).unwrap();
        std::fs::write(&p, b"abd").unwrap();
        assert!(matches!(verify_file(&p, &entry), Err(Error::HashMismatch { .. })));
        std::fs::write(&p, b"abcd").unwrap();
        assert!(verify_file(&p, &entry).is_err());
    }
}
