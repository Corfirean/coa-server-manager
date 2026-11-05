//! Sources of a module at an exact commit, downloaded as GitHub's archive of that commit (no git needed).
//!
//! The cache layout is the one the rest of the Manager uses, `<cache>/github/<owner>/<repo>/<sha>`, so a
//! module identifies itself by its repository name and a later rebuild finds the same tree.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Result};

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s != "." && s != ".."
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `<cache>/github/<owner>/<repo>/<sha>`, downloading it when it is not there yet.
pub fn fetch_pinned(owner: &str, repo: &str, sha: &str, cache_root: &Path) -> Result<PathBuf> {
    if !valid_name(owner) || !valid_name(repo) || sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid(format!("Invalid module source {owner}/{repo}@{sha}")));
    }
    let sha = sha.to_ascii_lowercase();
    let dest = cache_root.join("github").join(owner).join(repo).join(&sha);
    if dest.join(".coa_tree_digest").is_file() {
        return Ok(dest);
    }
    let url = format!("https://codeload.github.com/{owner}/{repo}/tar.gz/{sha}");
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let reply = client.get(&url).send().map_err(|e| Error::NetworkUnreachable(format!("{url}: {e}")))?;
    if !reply.status().is_success() {
        return Err(Error::Invalid(format!("GitHub answered {} for {owner}/{repo} at {sha}", reply.status())));
    }
    let staging = cache_root.join("staging").join(format!("{owner}_{repo}_{sha}_{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    let result = (|| -> Result<()> {
        unpack(reply.take(MAX_ARCHIVE_BYTES + 1), &staging)?;
        crate::analyzer::validate_source_tree_safety(&staging)?;
        let digest = crate::analyzer::compute_tree_digest(&staging)?;
        fs::write(staging.join(".coa_tree_digest"), digest)?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    fs::create_dir_all(dest.parent().unwrap())?;
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::rename(&staging, &dest)?;
    Ok(dest)
}

/// Extracts a GitHub tar.gz (one top-level `<repo>-<sha>/` folder) into `dest`, files and folders only.
fn unpack(reader: impl Read, dest: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(reader));
    let mut total = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let rel: PathBuf = path.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(Error::PathRejected(format!("archive entry {}", path.display())));
        }
        let kind = entry.header().entry_type();
        let target = dest.join(&rel);
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
        } else if kind.is_file() {
            total += entry.header().size()?;
            if total > MAX_ARCHIVE_BYTES {
                return Err(Error::Invalid("The module archive is too large".into()));
            }
            fs::create_dir_all(target.parent().unwrap())?;
            let mut out = fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut out)?;
        }
        // Links and special entries are not part of a module's sources: skipped.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (name, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut h, name, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn archive_is_unpacked_without_its_top_folder() {
        let dir = tempfile::tempdir().unwrap();
        let gz = tar_gz(&[("mod-x-abc/src/a.cpp", b"int a;"), ("mod-x-abc/conf/x.conf.dist", b"X = 1")]);
        unpack(&gz[..], dir.path()).unwrap();
        assert_eq!(fs::read(dir.path().join("src/a.cpp")).unwrap(), b"int a;");
        assert!(dir.path().join("conf/x.conf.dist").is_file());
    }

    #[test]
    fn bad_names_are_refused_before_any_download() {
        let dir = tempfile::tempdir().unwrap();
        assert!(fetch_pinned("../x", "repo", &"a".repeat(40), dir.path()).is_err());
        assert!(fetch_pinned("owner", "repo", "main", dir.path()).is_err());
    }
}
