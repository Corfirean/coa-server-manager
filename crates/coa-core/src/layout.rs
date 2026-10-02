//! Read-only detection and classification of a CoA Repack-shaped server folder.
//!
//! Nothing here writes to the scanned folder. Files are opened for reading only, and only the few files whose
//! content matters (server executables) are hashed; everything else is looked at by name and size.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::fsx;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Classification {
    Healthy,
    Partial,
    UnknownCustom,
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Found,
    Missing,
    Attention,
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub key: &'static str,
    pub label: &'static str,
    pub status: Status,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExeInfo {
    pub size: u64,
    pub sha256: String,
    /// Some(true/false) when RELEASE.json lists this binary, None when it does not.
    pub matches_release: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseJson {
    pub name: Option<String>,
    pub release_date: Option<String>,
    pub main_revision: Option<String>,
    pub source_revision: Option<String>,
    #[serde(default)]
    pub binaries: Vec<ReleaseBinary>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ReleaseBinary {
    pub name: String,
    #[serde(rename = "SHA256")]
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ports {
    pub mysql: u16,
    pub auth: u16,
    pub world: u16,
    pub ra: u16,
}

impl Default for Ports {
    fn default() -> Self {
        Ports { mysql: 3307, auth: 3724, world: 8085, ra: 3443 }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub path: String,
    pub executable: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub path: String,
    pub classification: Classification,
    pub items: Vec<Item>,
    pub worldserver: Option<ExeInfo>,
    pub authserver: Option<ExeInfo>,
    pub release: Option<ReleaseJson>,
    /// Core commit as printed by the worldserver banner (12 hex) when found in its log.
    pub banner_revision: Option<String>,
    pub module_configs: Vec<String>,
    pub bot_config_keys: usize,
    pub ports: Ports,
    pub database_schemas: Vec<String>,
    pub client: Option<ClientInfo>,
    pub notes: Vec<String>,
    /// When the chosen folder is not a server but a repack's main folder is right next to it (its parent, or a folder
    /// inside it): the folder the person most likely meant.
    pub suggested_path: Option<String>,
    /// "not-repack": a server was found but it is not laid out like the CoA Repack the Manager works with.
    pub hint: Option<&'static str>,
    /// Always false: a scan never modifies anything.
    pub modifies_files: bool,
}

pub(crate) fn exists(root: &Path, rel: &str) -> bool {
    root.join(rel).exists()
}

pub(crate) fn item(key: &'static str, label: &'static str, present: bool, detail: Option<String>) -> Item {
    Item { key, label, status: if present { Status::Found } else { Status::Missing }, detail }
}

pub(crate) fn hash_exe(path: &Path, release: Option<&ReleaseJson>, name: &str) -> Option<ExeInfo> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let sha256 = fsx::sha256_file(path).ok()?;
    let matches_release = release.and_then(|r| {
        r.binaries.iter().find(|b| b.name.eq_ignore_ascii_case(name)).map(|b| b.sha256.eq_ignore_ascii_case(&sha256))
    });
    Some(ExeInfo { size: meta.len(), sha256, matches_release })
}

pub fn read_ports(root: &Path) -> Ports {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct P {
        mysql_port: Option<u16>,
        auth_port: Option<u16>,
        world_port: Option<u16>,
        ra_port: Option<u16>,
    }
    let d = Ports::default();
    match fsx::read_json::<P>(&root.join("Settings/repack.json")) {
        Ok(p) => Ports {
            mysql: p.mysql_port.unwrap_or(d.mysql),
            auth: p.auth_port.unwrap_or(d.auth),
            world: p.world_port.unwrap_or(d.world),
            ra: p.ra_port.unwrap_or(d.ra),
        },
        Err(_) => d,
    }
}

/// Find the last worldserver banner `AzerothCore rev. <hex>` in a (possibly huge) log: the tail window first,
/// then the head window, since a long-running server's banner can be far from the end.
pub fn banner_revision_in_log(log: &Path) -> Option<String> {
    const WINDOW: u64 = 4 * 1024 * 1024;
    let len = File::open(log).ok()?.metadata().ok()?.len();
    let read = |start: u64| -> Option<String> {
        let mut f = File::open(log).ok()?;
        f.seek(SeekFrom::Start(start)).ok()?;
        let mut buf = Vec::new();
        f.take(WINDOW).read_to_end(&mut buf).ok()?;
        last_banner(&String::from_utf8_lossy(&buf))
    };
    read(len.saturating_sub(WINDOW)).or_else(|| if len > WINDOW { read(0) } else { None })
}

fn last_banner(text: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        if let Some(pos) = line.find("AzerothCore rev. ") {
            let rest = &line[pos + "AzerothCore rev. ".len()..];
            let rev: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            if rev.len() >= 7 {
                found = Some(rev);
            }
        }
    }
    found
}

pub fn detect_client(dir: &Path) -> Option<ClientInfo> {
    if !dir.join("Data").is_dir() {
        return None;
    }
    for exe in ["Ascension.exe", "Wow.exe", "WoW.exe"] {
        if dir.join(exe).is_file() {
            return Some(ClientInfo { path: dir.to_string_lossy().into_owned(), executable: exe.into() });
        }
    }
    None
}

pub(crate) fn count_bot_keys(conf: &Path) -> usize {
    fs::read_to_string(conf)
        .map(|t| t.lines().filter(|l| l.trim_start().starts_with("CoaBots.")).count())
        .unwrap_or(0)
}

/// Scan `root` without modifying it.
pub fn scan(root: &Path) -> Result<ScanReport> {
    if !root.is_dir() {
        return Err(crate::error::Error::Invalid(format!("{} is not a folder", root.display())));
    }
    let root = fsx::canonicalize_lenient(root)?;
    if crate::docker::is_docker(&root) {
        return crate::docker::scan(&root);
    }
    let mut notes = Vec::new();

    let release: Option<ReleaseJson> = fsx::read_json(&root.join("RELEASE.json")).ok();
    let world = hash_exe(&root.join("Core/worldserver.exe"), release.as_ref(), "worldserver.exe");
    let auth = hash_exe(&root.join("Core/authserver.exe"), release.as_ref(), "authserver.exe");

    let modules_dir = root.join("Core/configs/modules");
    let mut module_configs: Vec<String> = fs::read_dir(&modules_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.ends_with(".conf"))
                .collect()
        })
        .unwrap_or_default();
    module_configs.sort();

    let mysql_data = root.join("mysql/data");
    let mut database_schemas: Vec<String> = ["acore_auth", "acore_characters", "acore_world"]
        .iter()
        .filter(|s| mysql_data.join(s).is_dir())
        .map(|s| s.to_string())
        .collect();
    database_schemas.sort();

    // A fresh repack ships `mod_coa_playerbots.conf.dist` and only writes the active file when someone changes it.
    let bot_active = modules_dir.join("mod_coa_playerbots.conf");
    let bot_dist = modules_dir.join("mod_coa_playerbots.conf.dist");
    let bot_conf = if bot_active.is_file() { bot_active } else { bot_dist };
    let bot_config_keys = count_bot_keys(&bot_conf);
    let banner_revision = banner_revision_in_log(&root.join("Core/Logs/Server.log"));

    let has_repack_scaffold =
        exists(&root, "Scripts/manage.py") && exists(&root, "Settings/repack.json") && exists(&root, "Runtime/python/python.exe");
    let has_bundled_db = exists(&root, "mysql/bin/mysqld.exe") && database_schemas.len() == 3;
    let has_data = exists(&root, "Data/dbc") && exists(&root, "Data/maps");

    let items = vec![
        item("worldserver", "World server", world.is_some(), None),
        item("authserver", "Auth server", auth.is_some(), None),
        item("worldserver_conf", "World server configuration", exists(&root, "Core/configs/worldserver.conf"), None),
        item("authserver_conf", "Auth server configuration", exists(&root, "Core/configs/authserver.conf"), None),
        item("modules_conf", "Module configuration", modules_dir.is_dir(), Some(format!("{} files", module_configs.len()))),
        item("game_data", "Game data (maps, DBC)", has_data, None),
        item("database_runtime", "Database runtime", exists(&root, "mysql/bin/mysqld.exe"), None),
        item(
            "database_content",
            "Auth / Characters / World databases",
            has_bundled_db,
            Some(database_schemas.join(", ")),
        ),
        item("launcher", "Repack launcher scripts", has_repack_scaffold, None),
        item("release_info", "RELEASE.json", release.is_some(), release.as_ref().and_then(|r| r.main_revision.clone())),
        item(
            "companions",
            "CoA Companions (bots)",
            bot_conf.is_file(),
            (bot_config_keys > 0).then(|| format!("{bot_config_keys} settings")),
        ),
    ];

    let client = ["Client", "client"].iter().find_map(|d| detect_client(&root.join(d)));

    if let (Some(w), Some(r)) = (&world, &release) {
        if w.matches_release == Some(false) {
            notes.push(format!(
                "worldserver.exe differs from the one listed in RELEASE.json (release {}); this looks like a customised build.",
                r.release_date.clone().unwrap_or_default()
            ));
        }
    }
    if module_configs.iter().any(|c| c.ends_with(".bak")) {
        notes.push("Backup copies of module configs were found.".into());
    }

    let has_exes = world.is_some() && auth.is_some();
    let looks_like_client = root.join("Wow.exe").is_file() || root.join("Ascension.exe").is_file();
    let nested_repack = exists(&root, "Core/Core");

    let classification = if looks_like_client || nested_repack {
        Classification::Incompatible
    } else if has_exes && !has_repack_scaffold {
        Classification::UnknownCustom
    } else if has_exes && has_repack_scaffold && has_bundled_db && has_data
        && exists(&root, "Core/configs/worldserver.conf")
        && exists(&root, "Core/configs/authserver.conf")
    {
        Classification::Healthy
    } else if has_repack_scaffold || exists(&root, "Core") {
        Classification::Partial
    } else {
        Classification::Incompatible
    };

    if looks_like_client {
        notes.push("This looks like a game client folder, not a server folder.".into());
    }

    let (suggested_path, hint) = if classification == Classification::Incompatible && !looks_like_client {
        match find_repack_nearby(&root) {
            Some(p) => (Some(p.to_string_lossy().into_owned()), None),
            None if root.join("worldserver.exe").is_file() || has_exes => (None, Some("not-repack")),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    Ok(ScanReport {
        path: root.to_string_lossy().into_owned(),
        classification,
        items,
        worldserver: world,
        authserver: auth,
        release,
        banner_revision,
        module_configs,
        bot_config_keys,
        ports: read_ports(&root),
        database_schemas,
        client,
        notes,
        suggested_path,
        hint,
        modifies_files: false,
    })
}

fn is_repack_root(p: &Path) -> bool {
    p.join("Core/worldserver.exe").is_file() && p.join("Scripts/manage.py").is_file()
}

/// People often pick the `Core` folder (it holds worldserver.exe) or the folder an archive was unpacked into, with the real
/// server one level inside. Look at the parent, then at the folders inside (two levels), for the repack's main folder.
fn find_repack_nearby(root: &Path) -> Option<PathBuf> {
    if let Some(parent) = root.parent() {
        if is_repack_root(parent) {
            return Some(parent.to_path_buf());
        }
    }
    let subdirs = |dir: &Path| -> Vec<PathBuf> {
        fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).take(200).collect()).unwrap_or_default()
    };
    let first = subdirs(root);
    if let Some(p) = first.iter().find(|p| is_repack_root(p)) {
        return Some(p.clone());
    }
    first.iter().flat_map(|d| subdirs(d)).find(|p| is_repack_root(p))
}

/// Paths of the executables used by an installation, if present.
pub fn executables(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (root.join("Core/worldserver.exe"), root.join("Core/authserver.exe"), root.join("mysql/bin/mysqld.exe"))
}

#[cfg(any(test, feature = "testkit"))]
pub mod testkit {
    use std::fs;
    use std::path::Path;

    fn put(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    /// Build a minimal repack-shaped tree with fake binaries.
    pub fn fake_repack(root: &Path) {
        put(root, "Core/worldserver.exe", b"fake-world");
        put(root, "Core/authserver.exe", b"fake-auth");
        put(root, "Core/configs/worldserver.conf", b"[worldserver]\nRealmID = 1\n");
        put(root, "Core/configs/authserver.conf", b"[authserver]\n");
        put(root, "Core/configs/modules/mod_coa_playerbots.conf", b"CoaBots.AutoLoginOnStartup = 0\nCoaBots.X = 1\n");
        put(root, "Core/Logs/Server.log", b"x\nINFO AzerothCore rev. 3567e2f8e9d5 2026-09-18 ready...\n");
        put(root, "Data/dbc/Spell.dbc", b"d");
        put(root, "Data/maps/000.map", b"m");
        put(root, "mysql/bin/mysqld.exe", b"fake-mysqld");
        for db in ["acore_auth", "acore_characters", "acore_world"] {
            fs::create_dir_all(root.join("mysql/data").join(db)).unwrap();
        }
        put(root, "Scripts/manage.py", b"# stub");
        put(root, "Runtime/python/python.exe", b"stub");
        put(root, "Settings/repack.json", br#"{"mysqlPort":3307,"authPort":3724,"worldPort":8085,"raPort":3443}"#);
        let world_hash = crate::fsx::sha256_bytes(b"fake-world");
        let release = format!(
            r#"{{"name":"CoA Repack","releaseDate":"2026-09-11","mainRevision":"c3beca68","binaries":[{{"Name":"worldserver.exe","SHA256":"{world_hash}","Bytes":10}}]}}"#
        );
        put(root, "RELEASE.json", release.as_bytes());
        put(root, "user-notes.txt", b"my own file");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_repack_is_classified_and_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repack");
        testkit::fake_repack(&root);
        let before = tree(&root);
        let r = scan(&root).unwrap();
        assert_eq!(r.classification, Classification::Healthy);
        assert_eq!(r.banner_revision.as_deref(), Some("3567e2f8e9d5"));
        assert_eq!(r.bot_config_keys, 2);
        assert_eq!(r.worldserver.as_ref().unwrap().matches_release, Some(true));
        assert_eq!(r.authserver.as_ref().unwrap().matches_release, None);
        assert!(!r.modifies_files);
        assert_eq!(before, tree(&root), "scan must not modify anything");
    }

    #[test]
    fn customised_binary_is_flagged_but_still_healthy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repack");
        testkit::fake_repack(&root);
        fs::write(root.join("Core/worldserver.exe"), b"my-custom-build").unwrap();
        let r = scan(&root).unwrap();
        assert_eq!(r.classification, Classification::Healthy);
        assert_eq!(r.worldserver.unwrap().matches_release, Some(false));
        assert!(r.notes.iter().any(|n| n.contains("customised")));
    }

    #[test]
    fn partial_when_database_content_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repack");
        testkit::fake_repack(&root);
        fs::remove_dir_all(root.join("mysql/data/acore_world")).unwrap();
        assert_eq!(scan(&root).unwrap().classification, Classification::Partial);
    }

    #[test]
    fn unknown_custom_when_exes_without_repack_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("custom");
        fs::create_dir_all(root.join("Core")).unwrap();
        fs::write(root.join("Core/worldserver.exe"), b"w").unwrap();
        fs::write(root.join("Core/authserver.exe"), b"a").unwrap();
        assert_eq!(scan(&root).unwrap().classification, Classification::UnknownCustom);
    }

    #[test]
    fn the_core_folder_or_an_outer_folder_points_at_the_real_server_folder() {
        let dir = tempfile::tempdir().unwrap();
        let repack = dir.path().join("CoA-Repack");
        testkit::fake_repack(&repack);
        let core = scan(&repack.join("Core")).unwrap();
        assert_eq!(core.classification, Classification::Incompatible);
        assert!(Path::new(core.suggested_path.as_deref().unwrap()).ends_with("CoA-Repack"));

        let outer = tempfile::tempdir().unwrap();
        testkit::fake_repack(&outer.path().join("unpacked").join("CoA-Repack"));
        let r = scan(outer.path()).unwrap();
        assert_eq!(r.classification, Classification::Incompatible);
        assert!(Path::new(r.suggested_path.as_deref().unwrap()).ends_with("CoA-Repack"), "found two levels down");

        let healthy = scan(&repack).unwrap();
        assert!(healthy.suggested_path.is_none() && healthy.hint.is_none());
    }

    #[test]
    fn a_plain_azerothcore_folder_is_told_apart_from_a_wrong_folder() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("acore");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("worldserver.exe"), b"w").unwrap();
        let r = scan(&plain).unwrap();
        assert_eq!(r.classification, Classification::Incompatible);
        assert!(r.suggested_path.is_none());
        assert_eq!(r.hint, Some("not-repack"));
    }

    #[test]
    fn the_companions_are_found_by_their_documented_default_file_when_no_active_one_exists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repack");
        testkit::fake_repack(&root);
        let modules = root.join("Core/configs/modules");
        fs::rename(modules.join("mod_coa_playerbots.conf"), modules.join("mod_coa_playerbots.conf.dist")).unwrap();
        let r = scan(&root).unwrap();
        assert!(r.items.iter().any(|i| i.key == "companions" && i.status == Status::Found), "found through the .dist");
        assert_eq!(r.bot_config_keys, 2);
    }

    #[test]
    fn missing_folder_is_an_error_not_a_verdict() {
        assert!(scan(Path::new("C:/definitely/not/here")).is_err());
    }

    #[test]
    fn client_and_random_folders_are_incompatible() {
        let dir = tempfile::tempdir().unwrap();
        let client = dir.path().join("wow");
        fs::create_dir_all(client.join("Data")).unwrap();
        fs::write(client.join("Wow.exe"), b"x").unwrap();
        assert_eq!(scan(&client).unwrap().classification, Classification::Incompatible);
        let docs = dir.path().join("Documents");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("cv.docx"), b"x").unwrap();
        assert_eq!(scan(&docs).unwrap().classification, Classification::Incompatible);
    }

    #[test]
    fn banner_takes_the_last_occurrence() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Server.log");
        fs::write(&p, "AzerothCore rev. aaaaaaaaaaaa x\nnoise\nAzerothCore rev. bbbbbbbbbbbb y\n").unwrap();
        assert_eq!(banner_revision_in_log(&p).as_deref(), Some("bbbbbbbbbbbb"));
        assert_eq!(banner_revision_in_log(&dir.path().join("none")), None);
    }

    fn tree(root: &Path) -> Vec<(String, u64)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, u64)>) {
            for e in fs::read_dir(dir).unwrap() {
                let e = e.unwrap();
                let md = e.metadata().unwrap();
                out.push((e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned(), md.len()));
                if md.is_dir() {
                    walk(&e.path(), root, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }
}
