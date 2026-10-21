//! Fetching a signed package (manifest + signature + parts) from a URL or a local folder, shared by
//! installation and updates.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};

use crate::download::{self, Cancel, Job, Progress};
use crate::error::{Error, Result};
use crate::fsx;
use crate::manifest::Manifest;
use crate::signing;

/// Where a package comes from.
#[derive(Debug, Clone)]
pub enum Source {
    /// `https://.../` base URL holding manifest.json, manifest.json.sig and the parts.
    Url(String),
    /// A folder produced by `package::build` plus its `manifest.json.sig` (offline / local installs).
    Dir(PathBuf),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Channel {
    pub schema: u32,
    pub version: String,
    pub release: String,
    pub manifest_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_sha256: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct SignedChannel {
    pub payload: String,
    pub signature: String,
}

pub fn verify_channel(bytes: &[u8], trusted_key: &str) -> Result<Channel> {
    let signed: SignedChannel = serde_json::from_slice(bytes).map_err(|e| Error::InvalidManifest(e.to_string()))?;
    let payload = STANDARD.decode(&signed.payload).map_err(|e| Error::InvalidManifest(e.to_string()))?;
    signing::verify(&payload, &signed.signature, trusted_key)?;
    let pointer: Channel = serde_json::from_slice(&payload).map_err(|e| Error::InvalidManifest(e.to_string()))?;
    let required = format!("https://github.com/Corfirean/coa-server-build/releases/download/server-{}/", pointer.version);
    if pointer.schema != 1 || crate::manifest::parse_version(&pointer.version).is_none() || pointer.release != required {
        return Err(Error::InvalidManifest("Channel does not point to an immutable server release.".into()));
    }
    Ok(pointer)
}

/// A channel is one atomically replaced signed envelope. Its target is immutable.
pub struct PinnedPackage {
    pub source: Source,
    pub manifest: Manifest,
    pub bytes: Vec<u8>,
    pub signature: Vec<u8>,
}

pub fn fetch_package(source: &Source, trusted_key: &str) -> Result<PinnedPackage> {
    let channel = match source {
        Source::Dir(d) if !d.join("channel.json").exists() => None,
        _ => match fetch_small(source, "channel.json") {
            Ok(bytes) => Some(bytes),
            Err(Error::PackageNotPublished(_)) => None,
            Err(e) => return Err(e),
        },
    };
    let (mut resolved, expected) = if let Some(bytes) = channel {
        let pointer = verify_channel(&bytes, trusted_key)?;
        (Source::Url(pointer.release.clone()), Some(pointer))
    } else { (source.clone(), None) };
    let mut bytes = fetch_small(&resolved, "manifest.json")?;
    let mut signature = fetch_small(&resolved, "manifest.json.sig")?;
    signing::verify(&bytes, &String::from_utf8_lossy(&signature), trusted_key)?;
    let manifest = Manifest::parse(&bytes)?;
    // Legacy moving channels are pinned by matching their signed manifest to the immutable tag.
    if matches!(&resolved, Source::Url(url) if matches!(url.trim_end_matches('/'), "https://github.com/Corfirean/coa-server-build/releases/download/stable" | "https://github.com/Corfirean/coa-server-build/releases/download/edge")) {
        resolved = Source::Url(format!("https://github.com/Corfirean/coa-server-build/releases/download/server-{}/", manifest.version));
        let immutable = fetch_small(&resolved, "manifest.json")?;
        let sig = fetch_small(&resolved, "manifest.json.sig")?;
        signing::verify(&immutable, &String::from_utf8_lossy(&sig), trusted_key)?;
        if immutable != bytes { return Err(Error::InvalidManifest("Moving channel differs from its immutable release.".into())); }
        bytes = immutable;
        signature = sig;
    }
    if let Some(pointer) = expected {
        if pointer.version != manifest.version || pointer.manifest_sha256 != fsx::sha256_bytes(&bytes) {
            return Err(Error::InvalidManifest("Immutable release does not match the signed channel.".into()));
        }
    }
    Ok(PinnedPackage { source: resolved, manifest, bytes, signature })
}

/// Read a small file (manifest, signature) from the source.
pub fn fetch_small(source: &Source, name: &str) -> Result<Vec<u8>> {
    match source {
        Source::Dir(d) => Ok(fs::read(fsx::safe_join(d, name)?).map_err(|_| Error::Invalid(format!("{name} was not found in the package folder.")))?),
        Source::Url(base) => {
            let url = format!("{}/{name}", base.trim_end_matches('/'));
            download::check_url(&url)?;
            let t = download::HttpTransport::new()?;
            let reply = download::Transport::get(&t, &url, 0).map_err(|e| Error::NetworkUnreachable(format!("{e}")))?;
            if reply.status == 404 || reply.status == 410 {
                return Err(Error::PackageNotPublished(format!("the download server answered {} for {name} ({url})", reply.status)));
            }
            if reply.status != 200 {
                return Err(Error::Invalid(format!("The download server answered {} for {name}.", reply.status)));
            }
            let mut buf = Vec::new();
            reply.body.take(64 * 1024 * 1024).read_to_end(&mut buf)?;
            Ok(buf)
        }
    }
}

/// Download and verify the manifest. Returns the parsed manifest and its exact bytes.
pub fn fetch_manifest(source: &Source, trusted_key: &str) -> Result<(Manifest, Vec<u8>)> {
    let pinned = fetch_package(source, trusted_key)?;
    Ok((pinned.manifest, pinned.bytes))
}

/// Make every archive part available in a verified folder: a local source is used in place, a remote one is
/// downloaded (resumable) into `download_dir`. `report(fraction 0..1, speed_text)`.
/// 1536 -> "1.5 KB", 6.3e9 -> "5.9 GB" (binary units, as drive sizes are shown).
pub fn human_bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 || v >= 100.0 { format!("{} {}", v.round() as u64, units[i]) } else { format!("{v:.1} {}", units[i]) }
}

pub fn fetch_parts(source: &Source, manifest: &Manifest, download_dir: &Path, cancel: &Cancel, report: &dyn Fn(f64, Option<String>)) -> Result<PathBuf> {
    let archive = manifest.archive.as_ref().ok_or_else(|| Error::InvalidManifest("manifest has no archive".into()))?;
    match source {
        Source::Dir(d) => Ok(d.clone()),
        Source::Url(base) => {
            let all: u64 = archive.parts.iter().map(|p| p.size).sum::<u64>().max(1);
            let mut before = 0u64;
            for part in archive.parts.iter() {
                let job = Job { url: format!("{}/{}", base.trim_end_matches('/'), part.name), dest: download_dir.join(&part.name), sha256: part.sha256.clone(), size: part.size };
                download::fetch(&job, cancel, &|p: Progress| {
                    let done = before + p.downloaded.min(part.size);
                    // language-neutral so the screen can show it as it is: "3.1 GB / 5.9 GB · 40.6 MB/s"
                    report(done as f64 / all as f64, Some(format!("{} / {} \u{b7} {:.1} MB/s", human_bytes(done), human_bytes(all), p.bytes_per_sec as f64 / 1e6)));
                })?;
                before += part.size;
            }
            Ok(download_dir.to_path_buf())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_shown_in_binary_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(6 * (1 << 30)), "6.0 GB");
        assert_eq!(human_bytes(150 * (1 << 20)), "150 MB");
    }
}
