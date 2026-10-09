//! Base/update packages: a zstd-compressed tar stream cut into parts below the hosting size limit.
//!
//! `build` (used by the release pipeline) produces parts + `manifest.json`; `extract` (used by the Manager)
//! unpacks verified parts into a staging folder, accepting only regular files and directories that the signed
//! manifest lists, with matching size and SHA-256.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::fsx;
use crate::manifest::{
    ArchiveInfo, ArchivePart, FileEntry, Kind, Manifest, Owner, ReplacePolicy, Revision,
};

/// GitHub release assets are limited to 2 GiB; stay clearly below.
pub const DEFAULT_PART_SIZE: u64 = 1_900_000_000;

/// Archive path of the one-time database credentials shipped with a base package.
pub const BOOTSTRAP_CREDENTIALS: &str = "Settings/database.bootstrap.json";

/// Folder of a Linux (Docker) package that holds the starting databases: `<kind>.sql.zst` for `auth`, `characters`
/// and `world`, in the format of the Manager's own backups. They hold the state in which every migration of the package is
/// already applied (a world database cannot be rebuilt from the repository's SQL files: some migrations are guarded and
/// only apply to the maintainers' own database).
pub const BASELINE_DIR: &str = "Database/baseline";

/// Files that never belong in a shipped package: per-install state, logs, secrets, old binaries.
pub fn excluded(rel: &str) -> bool {
    if rel
        .replace('\\', "/")
        .to_lowercase()
        .starts_with(".realms/")
    {
        return true;
    }
    let l = rel.to_lowercase();
    let name = l.rsplit('/').next().unwrap_or("");
    l.starts_with(".state/")
        || l.starts_with("core/logs/")
        || l.starts_with("core/crashes/")
        || l.starts_with("bugreport/logs/")
        || l.starts_with("bugreport/reports/")
        || l.starts_with("mysql/logs/")
        || l.starts_with("source/")
        || l.starts_with("testing/")
        || l.starts_with("core_backup")
        || l.starts_with("scripts/__pycache__/")
        || l == "settings/database.json"
        || l == "mysql/admin-client.ini"
        || l == "mysql/my.ini"
        || l == "mysql/mysql.pid"
        || l == "mysql/data.7z"
        || l.starts_with("core/configs/") && !l.ends_with(".dist") && l != "core/configs/modules/playerbots.conf.settings.json" // generated at first start from Settings templates
        || name.ends_with(".pdb")
        || name.ends_with(".bak")
        || name.ends_with(".log")
        || name.contains(".pre-")
        || name.contains(".bak-")
        || name.ends_with(".orig")
        || name.contains(".prev_")
        || name.contains(".manager")
}

fn policy_for(rel: &str) -> (Owner, ReplacePolicy) {
    let l = rel.to_lowercase();
    if l.starts_with("_migrations/") {
        (Owner::Core, ReplacePolicy::NeverTouch) // staged for the migration runner, never copied into the server
    } else if l.ends_with(".dist") || l == "core/configs/modules/playerbots.conf.settings.json" {
        (Owner::Core, ReplacePolicy::Replace)
    } else if l.starts_with("mysql/data/") {
        (Owner::User, ReplacePolicy::NeverTouch)
    } else if l.starts_with("settings/") {
        (Owner::Core, ReplacePolicy::MergeConfig)
    } else if l.contains("/configs/") {
        (Owner::Core, ReplacePolicy::CreateIfMissing)
    } else {
        (Owner::Core, ReplacePolicy::Replace)
    }
}

struct SplitWriter {
    dir: PathBuf,
    max: u64,
    parts: Vec<ArchivePart>,
    cur: Option<(BufWriter<File>, Sha256, u64, String)>,
}

impl SplitWriter {
    fn new(dir: &Path, max: u64) -> Self {
        SplitWriter {
            dir: dir.to_path_buf(),
            max,
            parts: Vec::new(),
            cur: None,
        }
    }

    fn open_next(&mut self) -> io::Result<()> {
        let name = format!("base.tar.zst.{:03}", self.parts.len() + 1);
        let f = File::create(self.dir.join(&name))?;
        self.cur = Some((BufWriter::new(f), Sha256::new(), 0, name));
        Ok(())
    }

    fn close_current(&mut self) -> io::Result<()> {
        if let Some((mut w, h, n, name)) = self.cur.take() {
            w.flush()?;
            self.parts.push(ArchivePart {
                name,
                size: n,
                sha256: hex::encode(h.finalize()),
            });
        }
        Ok(())
    }
}

impl Write for SplitWriter {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        let total = buf.len();
        while !buf.is_empty() {
            if self.cur.is_none() {
                self.open_next()?;
            }
            let (w, h, n, _) = self.cur.as_mut().unwrap();
            let room = (self.max - *n) as usize;
            let take = room.min(buf.len());
            w.write_all(&buf[..take])?;
            h.update(&buf[..take]);
            *n += take as u64;
            buf = &buf[take..];
            if *n >= self.max {
                self.close_current()?;
            }
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.cur.as_mut() {
            Some((w, ..)) => w.flush(),
            None => Ok(()),
        }
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let ft = e.file_type()?;
        let rel = e
            .path()
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ft.is_symlink() {
            continue; // junctions / symlinks (e.g. a fixture's Data link) are never packaged
        }
        if ft.is_dir() {
            walk(root, &e.path(), out)?;
        } else if ft.is_file() && !excluded(&rel) {
            out.push(rel);
        }
    }
    Ok(())
}

pub struct BuildOptions {
    pub kind: Kind,
    pub version: String,
    pub core_commit: Option<String>,
    pub built_at: String,
    pub part_size: u64,
    pub bots_commit: Option<String>,
    pub migrations: Vec<crate::manifest::Migration>,
}

/// Package `src` into `out` (parts + manifest.json). Returns the manifest.
pub fn build(
    src: &Path,
    out: &Path,
    opts: &BuildOptions,
    progress: &dyn Fn(&str),
) -> Result<Manifest> {
    fs::create_dir_all(out)?;
    let mut rels = Vec::new();
    walk(src, src, &mut rels)?;
    // (name inside the archive, file on disk)
    let mut files: Vec<(String, PathBuf)> = rels
        .into_iter()
        .map(|r| (r.clone(), src.join(&r)))
        .collect();
    // The database users of the packaged data directory: needed once at install to rotate the passwords.
    let creds = src.join("Settings/database.json");
    if opts.kind == Kind::Base && creds.is_file() {
        files.push((BOOTSTRAP_CREDENTIALS.to_string(), creds));
    }
    files.sort();

    let split = SplitWriter::new(out, opts.part_size);
    let mut enc = zstd::Encoder::new(split, 6)?;
    enc.multithread(
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(2),
    )?;
    let mut tar = tar::Builder::new(enc);
    tar.mode(tar::HeaderMode::Deterministic);

    let mut entries = Vec::new();
    let mut unpacked = 0u64;
    for (i, (rel, path)) in files.iter().enumerate() {
        if i % 500 == 0 {
            progress(&format!("Packing {} / {}", i, files.len()));
        }
        fsx::safe_join(src, rel)?;
        let mut f = File::open(path)?;
        let size = f.metadata()?.len();
        let mut header = tar::Header::new_gnu();
        header.set_size(size);
        header.set_mode(0o755);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        // hash while streaming into the archive
        struct Tee<'a, R: Read> {
            inner: R,
            hasher: &'a mut Sha256,
        }
        impl<R: Read> Read for Tee<'_, R> {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                let n = self.inner.read(b)?;
                self.hasher.update(&b[..n]);
                Ok(n)
            }
        }
        let mut hasher = Sha256::new();
        tar.append_data(
            &mut header,
            rel,
            Tee {
                inner: &mut f,
                hasher: &mut hasher,
            },
        )?;
        let (owner, policy) = policy_for(rel);
        entries.push(FileEntry {
            path: rel.clone(),
            sha256: hex::encode(hasher.finalize()),
            size,
            owner,
            policy,
        });
        unpacked += size;
    }
    progress("Finishing archive");
    let enc = tar.into_inner()?;
    let mut split = enc.finish()?;
    split.close_current()?;

    let manifest = Manifest {
        schema: crate::manifest::SCHEMA,
        kind: opts.kind,
        version: opts.version.clone(),
        core: Revision {
            commit: opts.core_commit.clone(),
        },
        bots: opts
            .bots_commit
            .clone()
            .map(|c| Revision { commit: Some(c) }),
        built_at: opts.built_at.clone(),
        min_manager_version: "0.1.0".into(),
        files: entries,
        migrations: opts.migrations.clone(),
        archive: Some(ArchiveInfo {
            format: "tar.zst".into(),
            parts: split.parts.clone(),
            unpacked_size: unpacked,
        }),
    };
    manifest.validate()?;
    fsx::atomic_write(
        &out.join("manifest.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(manifest)
}

struct PartsReader {
    paths: Vec<PathBuf>,
    idx: usize,
    cur: Option<File>,
}

impl Read for PartsReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.cur.is_none() {
                match self.paths.get(self.idx) {
                    Some(p) => {
                        self.cur = Some(File::open(p)?);
                        self.idx += 1;
                    }
                    None => return Ok(0),
                }
            }
            let n = self.cur.as_mut().unwrap().read(buf)?;
            if n > 0 {
                return Ok(n);
            }
            self.cur = None;
        }
    }
}

/// Unpack verified `parts_dir` into `dest` (an empty or new staging folder). Every file must be listed in the
/// manifest with the recorded size and hash; anything else aborts the extraction.
pub fn extract(
    parts_dir: &Path,
    manifest: &Manifest,
    dest: &Path,
    progress: &dyn Fn(u64, u64),
) -> Result<()> {
    extract_selected(parts_dir, manifest, dest, &|_| true, progress)
}

/// Verify the entire archive, writing only selected files for a disposable release-schema fixture.
pub fn extract_selected(
    parts_dir: &Path,
    manifest: &Manifest,
    dest: &Path,
    select: &dyn Fn(&str) -> bool,
    progress: &dyn Fn(u64, u64),
) -> Result<()> {
    let archive = manifest
        .archive
        .as_ref()
        .ok_or_else(|| Error::InvalidManifest("manifest has no archive".into()))?;
    let mut paths = Vec::new();
    for part in &archive.parts {
        let p = parts_dir.join(&part.name);
        let len = fs::metadata(&p)
            .map_err(|_| Error::Invalid(format!("download part {} is missing", part.name)))?
            .len();
        if len != part.size {
            return Err(Error::HashMismatch {
                path: part.name.clone(),
                expected: format!("{} bytes", part.size),
                actual: format!("{len} bytes"),
            });
        }
        let actual = fsx::sha256_file(&p)?;
        if !actual.eq_ignore_ascii_case(&part.sha256) {
            return Err(Error::HashMismatch {
                path: part.name.clone(),
                expected: part.sha256.clone(),
                actual,
            });
        }
        paths.push(p);
    }
    let selected_size = manifest
        .files
        .iter()
        .filter(|f| select(&f.path))
        .map(|f| f.size)
        .sum();
    fsx::require_space(dest, selected_size)?;
    fs::create_dir_all(dest)?;

    let expected: HashMap<String, &FileEntry> = manifest
        .files
        .iter()
        .map(|f| (f.path.replace('\\', "/").to_lowercase(), f))
        .collect();
    let decoder = zstd::Decoder::new(PartsReader {
        paths,
        idx: 0,
        cur: None,
    })?;
    let mut tar = tar::Archive::new(decoder);
    let mut seen = std::collections::HashSet::new();
    let mut done = 0u64;
    for entry in tar.entries()? {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        let rel = entry.path()?.to_string_lossy().replace('\\', "/");
        match kind {
            tar::EntryType::Directory => continue,
            tar::EntryType::Regular | tar::EntryType::GNULongName | tar::EntryType::Continuous => {}
            other => {
                return Err(Error::PathRejected(format!(
                    "{rel}: unsupported archive entry type {other:?}"
                )))
            }
        }
        let key = rel.to_lowercase();
        let want = expected
            .get(&key)
            .ok_or_else(|| Error::PathRejected(format!("{rel}: not listed in the manifest")))?;
        if !seen.insert(key) {
            return Err(Error::PathRejected(format!(
                "{rel}: appears twice in the archive"
            )));
        }
        let target = fsx::safe_join(dest, &want.path)?;
        let selected = select(&want.path);
        let mut out = if selected {
            fs::create_dir_all(target.parent().unwrap())?;
            Some(BufWriter::new(File::create(&target)?))
        } else {
            None
        };
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut size = 0u64;
        let mut head = Vec::with_capacity(4);
        loop {
            let n = entry.read(&mut buf)?;
            if n == 0 {
                break;
            }
            if head.len() < 4 {
                head.extend_from_slice(&buf[..n.min(4 - head.len())]);
            }
            size += n as u64;
            if size > want.size {
                return Err(Error::HashMismatch {
                    path: rel,
                    expected: format!("{} bytes", want.size),
                    actual: "more".into(),
                });
            }
            hasher.update(&buf[..n]);
            if let Some(out) = out.as_mut() {
                out.write_all(&buf[..n])?;
            }
        }
        if let Some(out) = out.as_mut() {
            out.flush()?;
        }
        let actual = hex::encode(hasher.finalize());
        if size != want.size || !actual.eq_ignore_ascii_case(&want.sha256) {
            return Err(Error::HashMismatch {
                path: rel,
                expected: want.sha256.clone(),
                actual,
            });
        }
        // The archive does not carry per-file modes, so a Linux binary or script would be extracted unusable.
        // Recognise them by content (ELF header or `#!`); Windows has no execute bit and ignores this.
        #[cfg(unix)]
        if selected && (head.starts_with(b"\x7fELF") || head.starts_with(b"#!")) {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
        }
        done += size;
        progress(done, archive.unpacked_size);
    }
    if seen.len() != expected.len() {
        return Err(Error::InvalidManifest(format!(
            "archive holds {} files, manifest lists {}",
            seen.len(),
            expected.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(part: u64) -> BuildOptions {
        BuildOptions {
            kind: Kind::Base,
            version: "0.1.0".into(),
            core_commit: Some("a".repeat(40)),
            built_at: "2026-09-30T00:00:00Z".into(),
            part_size: part,
            bots_commit: None,
            migrations: vec![],
        }
    }

    fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut v = Vec::new();
        fn walk(dir: &Path, root: &Path, v: &mut Vec<(String, Vec<u8>)>) {
            for e in fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                if e.file_type().unwrap().is_dir() {
                    walk(&e.path(), root, v);
                } else {
                    v.push((
                        e.path()
                            .strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                        fs::read(e.path()).unwrap(),
                    ));
                }
            }
        }
        walk(root, root, &mut v);
        v.sort();
        v
    }

    #[test]
    fn upstream_settings_metadata_ships_and_updates_but_active_configs_stay_private() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        fs::create_dir_all(src.join("Core/configs/modules")).unwrap();
        fs::write(
            src.join("Core/configs/modules/playerbots.conf.settings.json"),
            "{}\n",
        )
        .unwrap();
        fs::write(
            src.join("Core/configs/modules/playerbots.conf"),
            "PlayerbotsDatabaseInfo = private\n",
        )
        .unwrap();
        let out = d.path().join("package");
        let manifest = build(
            &src,
            &out,
            &BuildOptions {
                kind: Kind::Update,
                version: "1.0.0".into(),
                core_commit: None,
                built_at: "test".into(),
                part_size: 1 << 20,
                bots_commit: None,
                migrations: vec![],
            },
            &|_| {},
        )
        .unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(
            manifest.files[0].path,
            "Core/configs/modules/playerbots.conf.settings.json"
        );
        assert_eq!(manifest.files[0].policy, ReplacePolicy::Replace);
    }

    fn source() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        crate::layout::testkit::fake_repack(&src);
        // noise that must be left out
        for (p, c) in [
            ("Core/Logs/Server.log", "log"),
            (".state/world.json", "{}"),
            ("Settings/database.json", "{\"rootPassword\":\"x\"}"),
            ("Core/worldserver.exe.pre-fix", "old"),
            ("Core/worldserver.pdb", "pdb"),
        ] {
            let f = src.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, c).unwrap();
        }
        // pseudo-random payload so compression yields several parts
        let mut x = 0x2545F4914F6CDD1Du64;
        let big: Vec<u8> = (0..300_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect();
        fs::write(src.join("Data/dbc/big.dbc"), big).unwrap();
        (d, src)
    }

    #[cfg(unix)]
    #[test]
    fn extract_makes_binaries_and_scripts_executable_and_nothing_else() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("src");
        for (p, c) in [
            ("Core/worldserver", &b"\x7fELF\x02\x01\x01 binary"[..]),
            ("Scripts/tool.sh", &b"#!/bin/sh\necho hi\n"[..]),
            ("Core/configs/worldserver.conf.dist", &b"Setting = 1\n"[..]),
        ] {
            let f = src.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, c).unwrap();
        }
        let out = d.path().join("out");
        let m = build(&src, &out, &opts(64 * 1024), &|_| {}).unwrap();
        let dest = d.path().join("dest");
        extract(&out, &m, &dest, &|_, _| {}).unwrap();
        let mode = |p: &str| fs::metadata(dest.join(p)).unwrap().permissions().mode() & 0o111;
        assert_ne!(mode("Core/worldserver"), 0, "ELF binary must be executable");
        assert_ne!(
            mode("Scripts/tool.sh"),
            0,
            "script with a shebang must be executable"
        );
        assert_eq!(
            mode("Core/configs/worldserver.conf.dist"),
            0,
            "plain files stay non-executable"
        );
    }

    #[test]
    fn selective_extraction_still_validates_unselected_files() {
        let (d, src) = source();
        let out = d.path().join("out");
        let mut manifest = build(&src, &out, &opts(64 * 1024), &|_| {}).unwrap();
        let dest = d.path().join("selected");
        extract_selected(
            &out,
            &manifest,
            &dest,
            &|p| p.starts_with("Core/"),
            &|_, _| {},
        )
        .unwrap();
        assert!(dest.join("Core/worldserver.exe").exists());
        assert!(!dest.join("Data").exists());
        manifest
            .files
            .iter_mut()
            .find(|f| f.path.starts_with("Data/"))
            .unwrap()
            .sha256 = "f".repeat(64);
        assert!(matches!(
            extract_selected(
                &out,
                &manifest,
                &d.path().join("bad"),
                &|_| false,
                &|_, _| {}
            ),
            Err(Error::HashMismatch { .. })
        ));
    }

    #[test]
    fn build_split_extract_roundtrip_excludes_noise_and_secrets() {
        let (d, src) = source();
        let out = d.path().join("out");
        let m = build(&src, &out, &opts(64 * 1024), &|_| {}).unwrap();
        let parts = &m.archive.as_ref().unwrap().parts;
        assert!(
            parts.len() >= 3,
            "small part size forces a split, got {}",
            parts.len()
        );
        assert!(parts.iter().all(|p| p.size <= 64 * 1024));
        let listed: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        for bad in [
            "Core/Logs/Server.log",
            ".state/world.json",
            "Settings/database.json",
            "Core/worldserver.pdb",
        ] {
            assert!(!listed.contains(&bad), "{bad} must not be packaged");
        }
        assert!(listed.contains(&"Core/worldserver.exe"));
        assert!(
            listed.contains(&BOOTSTRAP_CREDENTIALS) || !src.join("Settings/database.json").exists()
        );

        let dest = d.path().join("dest");
        extract(&out, &m, &dest, &|_, _| {}).unwrap();
        let mut expected = tree(&src);
        expected.retain(|(p, _)| listed.contains(&p.as_str()));
        // the source's database.json travels under its bootstrap name
        expected.push((
            BOOTSTRAP_CREDENTIALS.to_string(),
            fs::read(src.join("Settings/database.json")).unwrap(),
        ));
        expected.sort();
        assert_eq!(tree(&dest), expected);
        // user data policy
        assert!(m
            .files
            .iter()
            .filter(|f| f.path.starts_with("mysql/data/"))
            .all(|f| f.policy == ReplacePolicy::NeverTouch));
    }

    #[test]
    fn a_damaged_part_is_rejected_before_anything_is_written() {
        let (d, src) = source();
        let out = d.path().join("out");
        let m = build(&src, &out, &opts(64 * 1024), &|_| {}).unwrap();
        let p = out.join(&m.archive.as_ref().unwrap().parts[1].name);
        let mut b = fs::read(&p).unwrap();
        b[100] ^= 0x55;
        fs::write(&p, b).unwrap();
        let dest = d.path().join("dest");
        assert!(matches!(
            extract(&out, &m, &dest, &|_, _| {}),
            Err(Error::HashMismatch { .. })
        ));
        assert!(!dest.exists(), "nothing extracted");
    }

    /// Build a hostile archive by hand and run it through `extract` with a manifest that "allows" it.
    fn hostile(
        build_tar: impl FnOnce(&mut tar::Builder<zstd::Encoder<'static, Vec<u8>>>),
    ) -> (tempfile::TempDir, Manifest) {
        let d = tempfile::tempdir().unwrap();
        let mut tb = tar::Builder::new(zstd::Encoder::new(Vec::new(), 1).unwrap());
        build_tar(&mut tb);
        let bytes = tb.into_inner().unwrap().finish().unwrap();
        let name = "base.tar.zst.001";
        fs::write(d.path().join(name), &bytes).unwrap();
        let ok_sha = fsx::sha256_bytes(b"data");
        let m = Manifest {
            schema: 1,
            kind: Kind::Base,
            version: "0.1.0".into(),
            core: Revision { commit: None },
            bots: None,
            built_at: "x".into(),
            min_manager_version: "0.1.0".into(),
            files: vec![FileEntry {
                path: "ok.txt".into(),
                sha256: ok_sha,
                size: 4,
                owner: Owner::Core,
                policy: ReplacePolicy::Replace,
            }],
            migrations: vec![],
            archive: Some(ArchiveInfo {
                format: "tar.zst".into(),
                parts: vec![ArchivePart {
                    name: name.into(),
                    size: bytes.len() as u64,
                    sha256: fsx::sha256_bytes(&bytes),
                }],
                unpacked_size: 4,
            }),
        };
        (d, m)
    }

    fn add(
        tb: &mut tar::Builder<zstd::Encoder<'static, Vec<u8>>>,
        path: &str,
        data: &[u8],
        ty: tar::EntryType,
    ) {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_entry_type(ty);
        h.set_mode(0o644);
        // write the path bytes directly so `..` survives (set_path would refuse it)
        let name = &mut h.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        h.set_cksum();
        tb.append(&h, data).unwrap();
    }

    #[test]
    fn traversal_unlisted_and_symlink_entries_are_refused() {
        for (path, ty) in [
            ("../evil.txt", tar::EntryType::Regular),
            ("not-listed.txt", tar::EntryType::Regular),
            ("ok.txt", tar::EntryType::Symlink),
        ] {
            let (d, m) = hostile(|tb| add(tb, path, b"data", ty));
            let dest = d.path().join("dest");
            assert!(
                extract(d.path(), &m, &dest, &|_, _| {}).is_err(),
                "{path} {ty:?}"
            );
            assert!(!d.path().join("evil.txt").exists());
        }
    }

    #[test]
    fn content_that_does_not_match_the_manifest_is_refused() {
        let (d, m) = hostile(|tb| add(tb, "ok.txt", b"DATA", tar::EntryType::Regular));
        assert!(matches!(
            extract(d.path(), &m, &d.path().join("dest"), &|_, _| {}),
            Err(Error::HashMismatch { .. })
        ));
        let (d2, m2) = hostile(|tb| add(tb, "ok.txt", b"data", tar::EntryType::Regular));
        extract(d2.path(), &m2, &d2.path().join("dest"), &|_, _| {}).unwrap();
        assert_eq!(fs::read(d2.path().join("dest/ok.txt")).unwrap(), b"data");
    }
}
