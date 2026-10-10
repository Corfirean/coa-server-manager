use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::parser::ConfFile;
use crate::db::{Account, Db};
use crate::error::{Error, Result};
use crate::{fsx, layout, process};

const STATE: &str = "Settings/realm-profile.json";
const JOURNAL: &str = "Settings/realm-switch.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Coa,
    Wildcard,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Coa => "coa",
            Self::Wildcard => "wildcard",
        }
    }
    pub fn realm_id(self) -> u32 {
        if self == Self::Coa {
            1
        } else {
            2
        }
    }
    pub fn schema(self, kind: &str) -> Result<&'static str> {
        match (self, kind) {
            (_, "auth") => Ok("acore_auth"),
            (Self::Coa, "characters") => Ok("acore_characters"),
            (Self::Coa, "world") => Ok("acore_world"),
            (Self::Wildcard, "characters") => Ok("acore_characters_wildcard"),
            (Self::Wildcard, "world") => Ok("acore_world_wildcard"),
            _ => Err(Error::Invalid("Unknown realm database.".into())),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RealmState {
    pub active: Mode,
    pub wildcard_created: bool,
    #[serde(default)]
    pub simultaneous: bool,
    #[serde(default)]
    pub secondary_world_port: Option<u16>,
    #[serde(default)]
    pub secondary_ra_port: Option<u16>,
}

#[derive(Debug, Serialize)]
pub struct View {
    pub active: Mode,
    pub wildcard_created: bool,
    pub supported: bool,
    pub recovery_pending: bool,
    pub simultaneous: bool,
    pub secondary_world_port: Option<u16>,
    pub secondary_running: bool,
}

pub fn state(root: &Path) -> Result<RealmState> {
    if !root.join(STATE).exists() {
        return Ok(RealmState::default());
    }
    fsx::read_json(&root.join(STATE))
}

/// The world server executable of this folder: `worldserver.exe` in a repack, `worldserver` in a Docker server.
fn world_binary(root: &Path) -> std::path::PathBuf {
    let exe = root.join("Core/worldserver.exe");
    if exe.exists() { exe } else { root.join("Core/worldserver") }
}

pub fn view(root: &Path) -> Result<View> {
    let s = state(root)?;
    // Inspect the binary itself; templates or an old log cannot prove installed support.
    let supported = fs::read(world_binary(root))?
        .windows(b"Wildcard synergy settings".len())
        .any(|w| w == b"Wildcard synergy settings");
    Ok(View {
        active: s.active,
        wildcard_created: s.wildcard_created,
        supported,
        recovery_pending: root.join(JOURNAL).exists(),
        simultaneous: s.simultaneous,
        secondary_world_port: s.secondary_world_port,
        secondary_running: crate::multiworld::is_running(root),
    })
}

pub fn guard_module(root: &Path, id: &str) -> Result<()> {
    if state(root)?.active == Mode::Wildcard && matches!(id, "companions" | "playerbots") {
        return Err(Error::Invalid("CoA companion class mechanics have not been validated for Wildcard. This module is unavailable on this realm.".into()));
    }
    Ok(())
}

type Files = BTreeMap<String, Vec<u8>>;

fn configs(root: &Path) -> Result<Files> {
    let mut files = Files::new();
    for rel in crate::backup::config_files(root) {
        if rel.starts_with("Core/configs/") && rel.ends_with(".conf")
            || rel.starts_with("Settings/") && rel.ends_with(".template")
        {
            files.insert(rel.clone(), fs::read(fsx::safe_join(root, &rel)?)?);
        }
    }
    Ok(files)
}

fn snapshot(root: &Path, mode: Mode) -> std::path::PathBuf {
    root.join(format!("Settings/realm-profiles/{}.json", mode.name()))
}

fn write_configs(root: &Path, files: &Files) -> Result<()> {
    for rel in configs(root)?.keys() {
        if !files.contains_key(rel) {
            fs::remove_file(fsx::safe_join(root, rel)?)?;
        }
    }
    for (rel, bytes) in files {
        if !(rel.starts_with("Core/configs/") && rel.ends_with(".conf")
            || rel.starts_with("Settings/") && rel.ends_with(".template"))
        {
            return Err(Error::Invalid("Invalid realm configuration path.".into()));
        }
        fsx::atomic_write(
            &fsx::ensure_within(root, &fsx::safe_join(root, rel)?)?,
            bytes,
        )?;
    }
    Ok(())
}

fn set(files: &mut Files, path: &str, key: &str, value: &str) -> Result<()> {
    let mut conf = ConfFile::parse_bytes(
        files
            .entry(path.into())
            .or_insert_with(|| b"[worldserver]\n".to_vec()),
    )?;
    conf.set(key, value, &[]);
    files.insert(path.into(), conf.to_text().into_bytes());
    Ok(())
}

fn wildcard_configs(mut files: Files) -> Result<Files> {
    for (key, value) in [
        ("CoA.ClassModel", "\"hero\""),
        ("CoA.GameModeMask", "64"),
        ("CoA.MapClass10ToWarrior", "0"),
        ("CoA.RealmType", "\"live seasonal\""),
        (
            "CoA.ClientBooleanConfigs",
            "\"CONFIG_LEGACY_CHARACTER_ADVANCEMENT_ENABLED=0\"",
        ),
    ] {
        set(&mut files, "Core/configs/modules/coa.conf", key, value)?;
    }
    set(
        &mut files,
        "Core/configs/modules/mod-coa-challenges.conf",
        "CoAChallenges.GameModes.Realm",
        "WildCard",
    )?;
    set(
        &mut files,
        "Core/configs/modules/mod-coa-challenges.conf",
        "CoAChallenges.GameModes.Enable",
        "1",
    )?;
    for (key, value) in [
        ("ChancePercent", "65"),
        ("LinkWeight", "3"),
        ("RelatedWeight", "2"),
        ("SpecTagWeight", "2"),
        ("SchoolTagWeight", "1"),
        ("TooltipWeight", "3"),
        ("TalentsNeedTarget", "1"),
        ("LogRolls", "1"),
    ] {
        set(
            &mut files,
            "Core/configs/modules/wildcard.conf",
            &format!("Wildcard.Synergy.{key}"),
            value,
        )?;
    }
    for path in [
        "Settings/worldserver.conf.template",
        "Core/configs/worldserver.conf",
    ] {
        if files.contains_key(path) {
            for (key, value) in [
                ("RealmID", "2"),
                ("AlwaysMaxSkillForLevel", "1"),
                ("PlayerStart.CustomSpells", "1"),
            ] {
                set(&mut files, path, key, value)?;
            }
        }
    }
    for (key, value) in [("CoaBots.Enable", "0"), ("CoaBots.AutoLoginOnStartup", "0")] {
        set(
            &mut files,
            "Core/configs/modules/mod_coa_playerbots.conf",
            key,
            value,
        )?;
    }
    Ok(files)
}

#[derive(Serialize, Deserialize)]
struct Journal {
    before: RealmState,
    files: Files,
}

fn require_world_stopped(root: &Path) -> Result<()> {
    let observed = process::observe(root, &layout::read_ports(root));
    if observed.world.state != process::ServiceState::Stopped
        || observed.auth.state != process::ServiceState::Stopped
        || crate::multiworld::is_running(root)
    {
        return Err(Error::Invalid(
            "Stop the server before switching realms. Your characters will be saved.".into(),
        ));
    }
    Ok(())
}

pub fn recover(root: &Path) -> Result<()> {
    if !root.join(JOURNAL).exists() {
        return Ok(());
    }
    require_world_stopped(root)?;
    let journal: Journal = fsx::read_json(&root.join(JOURNAL))?;
    write_configs(root, &journal.files)?;
    fsx::atomic_write_json(&root.join(STATE), &journal.before)?;
    fs::remove_file(root.join(JOURNAL))?;
    Ok(())
}

fn create_databases(root: &Path) -> Result<()> {
    crate::backup::with_database(root, |_| {
        let db = Db::from_repack(root, Account::Admin)?.for_realm(Mode::Coa);
        if db.query("SELECT COUNT(*) FROM acore_auth.realmlist WHERE id=2;")? != "0" {
            return Err(Error::Invalid(
                "Realm ID 2 is already in use. Its settings were left intact.".into(),
            ));
        }
        for kind in ["world", "characters"] {
            let source = Mode::Coa.schema(kind)?;
            let dest = Mode::Wildcard.schema(kind)?;
            if db.schema_exists(dest)? {
                return Err(Error::Invalid(format!("Database {dest} already exists. It was left intact; inspect it before creating a realm.")));
            }
            if db.extra_objects(source)? != 0 {
                return Err(Error::Invalid(
                    "Realm creation does not support databases with routines, triggers or views."
                        .into(),
                ));
            }
        }
        for (kind, tables) in crate::schema_check::WILDCARD_TABLES {
            let schema = crate::db::schema_of(kind)?;
            let present = db.tables(schema)?;
            let missing: Vec<_> = tables
                .iter()
                .filter(|table| !present.iter().any(|p| p == **table))
                .copied()
                .collect();
            if !missing.is_empty() {
                tracing::error!(root = %root.display(), database = schema, missing_tables = ?missing, "Wildcard realm creation blocked by missing database tables");
                return Err(Error::Invalid(format!("Cannot create Wildcard: {schema} is missing tables: {}. Run Check files and database and include its report when requesting support.", missing.join(", "))));
            }
        }
        let cache = root.join("Settings/realm-profiles/staging");
        fs::create_dir_all(&cache)?;
        fsx::require_space(
            &cache,
            db.schema_bytes("acore_world")?.saturating_mul(2) + 512 * 1024 * 1024,
        )?;
        let dump = cache.join("world.sql.zst");
        db.dump_to("acore_world", &dump)?;
        let stage_world = format!("coa_realm_world_{}", uuid::Uuid::new_v4().simple());
        let stage_chars = format!("coa_realm_chars_{}", uuid::Uuid::new_v4().simple());
        db.query(&format!("CREATE DATABASE `{stage_world}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci; CREATE DATABASE `{stage_chars}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;"))?;
        db.import_from(&stage_world, &dump)?;
        db.clone_structure("acore_characters", &stage_chars, &cache)?;
        for table in [
            "active_arena_season",
            "addons",
            "updates",
            "updates_include",
            "warden_action",
        ] {
            db.query(&format!(
                "INSERT INTO `{stage_chars}`.`{table}` SELECT * FROM acore_characters.`{table}`;"
            ))?;
        }
        // The newly empty realm inherits schema migration history, never the owner's characters or cards.
        if db
            .tables("acore_world")?
            .iter()
            .any(|t| t == "coa_manager_migrations")
        {
            db.query(&format!("CREATE TABLE IF NOT EXISTS `{stage_world}`.coa_manager_migrations LIKE acore_world.coa_manager_migrations;"))?;
        }
        db.query("CREATE DATABASE acore_world_wildcard CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci; CREATE DATABASE acore_characters_wildcard CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;")?;
        let mut renames = Vec::new();
        for (stage, dest) in [
            (&stage_world, "acore_world_wildcard"),
            (&stage_chars, "acore_characters_wildcard"),
        ] {
            for table in db.tables(stage)? {
                if !crate::db::valid_identifier(&table) {
                    return Err(Error::Invalid("Unsupported table name.".into()));
                }
                renames.push(format!("`{stage}`.`{table}` TO `{dest}`.`{table}`"));
            }
        }
        db.query(&format!("RENAME TABLE {};", renames.join(", ")))?;
        for host in db
            .query("SELECT host FROM mysql.user WHERE user='acore';")?
            .lines()
        {
            let host = hex::encode(host.as_bytes());
            // Host names are trusted database data, but still quote them through a hex SQL literal.
            db.query(&format!("SET @host=CONVERT(UNHEX('{host}') USING utf8mb4); SET @grant=CONCAT('GRANT ALL ON acore_world_wildcard.* TO ', QUOTE('acore'), '@', QUOTE(@host)); PREPARE realm_grant FROM @grant; EXECUTE realm_grant; DEALLOCATE PREPARE realm_grant; SET @grant=CONCAT('GRANT ALL ON acore_characters_wildcard.* TO ', QUOTE('acore'), '@', QUOTE(@host)); PREPARE realm_grant FROM @grant; EXECUTE realm_grant; DEALLOCATE PREPARE realm_grant;"))?;
        }
        db.query(&format!(
            "DROP DATABASE `{stage_world}`; DROP DATABASE `{stage_chars}`;"
        ))?;
        fs::remove_file(&dump)?;
        Ok(())
    })
}

pub fn select(root: &Path, mode: Mode) -> Result<View> {
    let meta = crate::registry::metadata_dir_for(root)?;
    let _update_lock = crate::update::operation_lock(&meta)?;
    crate::update::ensure_recovered(&meta)?;
    require_world_stopped(root)?;
    recover(root)?;
    let mut s = state(root)?;
    if mode == s.active {
        return view(root);
    }
    if mode == Mode::Wildcard && !view(root)?.supported {
        return Err(Error::Invalid(
            "Update the server to a build with Wildcard support first.".into(),
        ));
    }
    // Verify launcher support before creating databases or changing the selected realm.
    prepare_launcher(root)?;
    let before = configs(root)?;
    fsx::atomic_write_json(&snapshot(root, s.active), &before)?;
    if mode == Mode::Wildcard && !s.wildcard_created {
        fsx::atomic_write_json(&snapshot(root, mode), &wildcard_configs(before.clone())?)?;
        create_databases(root)?;
        s.wildcard_created = true;
        fsx::atomic_write_json(&root.join(STATE), &s)?;
    }
    let target: Files = fsx::read_json(&snapshot(root, mode))?;
    // The launcher and batch files share this lock; recheck after the potentially long database copy.
    fs::create_dir_all(root.join(".state"))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".state/control.lock"))?;
    if !fs4::fs_std::FileExt::try_lock_exclusive(&lock)? {
        return Err(Error::Invalid(
            "Another start/stop action is in progress.".into(),
        ));
    }
    require_world_stopped(root)?;
    let original = state(root)?;
    fsx::atomic_write_json(
        &root.join(JOURNAL),
        &Journal {
            before: original,
            files: before,
        },
    )?;
    let result = (|| {
        write_configs(root, &target)?;
        s.active = mode;
        fsx::atomic_write_json(&root.join(STATE), &s)?;
        Ok(())
    })();
    if let Err(e) = result {
        drop(lock);
        recover(root)?;
        return Err(e);
    }
    fs::remove_file(root.join(JOURNAL))?;
    view(root)
}

/// Adapt the bundled launcher's canonical schema and realm choices without changing its supervision logic.
pub fn prepare_launcher(root: &Path) -> Result<()> {
    // A Docker server has no launcher to adapt: the Docker backend reads the active realm itself.
    if crate::docker::is_docker(root) { return Ok(()); }
    let path = root.join("Scripts/manage.py");
    let source = fs::read_to_string(&path)?;
    let patched = patch_launcher(&source)?;
    if patched != source {
        fsx::atomic_write(&path, patched.as_bytes())?;
        // A Manager-owned transformation is still pristine for subsequent updates and file checks.
        if let Ok(dir) = crate::registry::metadata_dir_for(root) {
            if let Ok((_, mut meta)) = crate::registry::MetaDir::open(&dir) {
                if meta
                    .original_hashes
                    .get("Scripts/manage.py")
                    .is_some_and(|hash| hash == &fsx::sha256_bytes(source.as_bytes()))
                {
                    meta.original_hashes.insert(
                        "Scripts/manage.py".into(),
                        fsx::sha256_bytes(patched.as_bytes()),
                    );
                    fsx::atomic_write_json(&dir.join("install.json"), &meta)?;
                }
            }
        }
    }
    Ok(())
}

const LAUNCHER_REALM_REPLACEMENTS: &[(&str, &str)] = &[
    ("(\"WorldDatabaseInfo\", \"acore_world\")", "(\"WorldDatabaseInfo\", \"acore_world_wildcard\" if coa_realm() == 'wildcard' else \"acore_world\")"),
    ("(\"CharacterDatabaseInfo\", \"acore_characters\")", "(\"CharacterDatabaseInfo\", \"acore_characters_wildcard\" if coa_realm() == 'wildcard' else \"acore_characters\")"),
    ("SET name='AzerothCore',address=", "SET name='{coa_realm_name()}',address="),
    ("WHERE id=1;", "WHERE id={coa_realm_id()};"),
    ("mysql(\"UPDATE acore_auth.realmlist SET flag=0", "mysql(f\"UPDATE acore_auth.realmlist SET flag=0"),
];

const LAUNCHER_REALM_INSERT: &str = "# coa-manager-realm-profiles-v1\ndef coa_realm():\n    path = ROOT / 'Settings/realm-profile.json'\n    return json.loads(path.read_text(encoding='utf-8')).get('active', 'coa') if path.exists() else 'coa'\n\ndef coa_realm_id():\n    return 2 if coa_realm() == 'wildcard' else 1\n\ndef coa_realm_name():\n    return 'Wildcard' if coa_realm() == 'wildcard' else 'Conquest of Azeroth'\n\n";

pub(crate) fn patch_launcher(source: &str) -> Result<String> {
    if source.contains("# coa-manager-realm-profiles-v1") {
        return Ok(source.into());
    }
    let mut out = source.to_string();
    for &(from, to) in LAUNCHER_REALM_REPLACEMENTS {
        if !out.contains(from) {
            return Err(Error::Invalid(
                "This launcher does not support realm profiles. Update the server package first."
                    .into(),
            ));
        }
        out = out.replace(from, to);
    }
    // Insert before the entry point so ROOT, json and functions are all defined when called.
    let marker = "if __name__ == \"__main__\":";
    if !out.contains(marker) {
        return Err(Error::Invalid(
            "Unrecognised server launcher entry point.".into(),
        ));
    }
    Ok(out.replace(marker, &format!("{LAUNCHER_REALM_INSERT}{marker}")))
}

/// Reverse only our exact realm transformation; callers must verify the recovered original hash.
pub(crate) fn unpatch_launcher(source: &str) -> Option<String> {
    if !source.contains(LAUNCHER_REALM_INSERT) {
        return None;
    }
    let mut original = source.replacen(LAUNCHER_REALM_INSERT, "", 1);
    for &(from, to) in LAUNCHER_REALM_REPLACEMENTS {
        original = original.replace(to, from);
    }
    patch_launcher(&original)
        .ok()
        .filter(|patched| patched == source)
        .map(|_| original)
}

pub fn before_start(root: &Path) -> Result<()> {
    recover(root)?;
    let s = state(root)?;
    if !root.join(STATE).exists() {
        return Ok(());
    }
    prepare_launcher(root)?;
    if s.simultaneous && !view(root)?.supported {
        return Err(Error::Invalid(
            "The installed server build does not support simultaneous Wildcard startup.".into(),
        ));
    }
    if s.simultaneous {
        fsx::atomic_write_json(&snapshot(root, s.active), &configs(root)?)?;
    }
    if s.active == Mode::Wildcard {
        if !view(root)?.supported {
            return Err(Error::Invalid(
                "The installed server build does not support Wildcard.".into(),
            ));
        }
        let path = root.join("Core/configs/modules/mod_coa_playerbots.conf");
        let mut conf = ConfFile::parse_bytes(&fs::read(&path)?)?;
        conf.set("CoaBots.Enable", "0", &[]);
        fsx::atomic_write(&path, conf.to_text().as_bytes())?;
    }
    Ok(())
}

pub fn setup_realmlist(root: &Path) -> Result<()> {
    let s = state(root)?;
    if !root.join(STATE).exists() || !s.wildcard_created {
        return Ok(());
    }
    let db = Db::from_repack(root, Account::Admin)?;
    let port = layout::read_ports(root).world;
    db.query(&format!("INSERT INTO acore_auth.realmlist (id,name,address,localAddress,localSubnetMask,port,icon,flag,timezone,gamebuild) SELECT 2,'Wildcard',address,localAddress,localSubnetMask,{port},icon,2,timezone,gamebuild FROM acore_auth.realmlist WHERE id=1 ON DUPLICATE KEY UPDATE port={port}; UPDATE acore_auth.realmlist SET flag=2 WHERE id IN (1,2) AND id<>{};", s.active.realm_id()))?;
    if s.simultaneous {
        let second = s
            .secondary_world_port
            .ok_or_else(|| Error::Invalid("Second realm port is missing.".into()))?;
        let (coa, wildcard) = if s.active == Mode::Coa {
            (port, second)
        } else {
            (second, port)
        };
        db.query(&format!("UPDATE acore_auth.realmlist SET port=CASE id WHEN 1 THEN {coa} ELSE {wildcard} END,flag=0 WHERE id IN (1,2);"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_docker_server_is_checked_for_wildcard_support_in_its_own_binary() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("Core")).unwrap();
        fs::create_dir_all(d.path().join("Settings")).unwrap();
        fs::write(d.path().join("Settings/docker.json"), br#"{"project":"t1"}"#).unwrap();
        fs::write(d.path().join("Core/worldserver"), b"ELF...Wildcard synergy settings...").unwrap();
        assert!(view(d.path()).unwrap().supported, "worldserver without .exe is read too");
        fs::write(d.path().join("Core/worldserver"), b"ELF...an older build").unwrap();
        assert!(!view(d.path()).unwrap().supported);
        // The repack's executable still wins when both exist, so a repack is read as before.
        fs::write(d.path().join("Core/worldserver.exe"), b"MZ...Wildcard synergy settings").unwrap();
        assert!(view(d.path()).unwrap().supported);
    }

    #[test]
    fn a_docker_server_has_no_launcher_to_adapt() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("Settings")).unwrap();
        // No Scripts/manage.py anywhere: on a repack this fails, on a Docker server there is nothing to do.
        assert!(prepare_launcher(d.path()).is_err());
        fs::write(d.path().join("Settings/docker.json"), br#"{"project":"t1"}"#).unwrap();
        prepare_launcher(d.path()).unwrap();
    }

    #[test]
    fn fresh_wildcard_disables_bots_and_preserves_scaling() {
        let mut f = Files::new();
        f.insert(
            "Core/configs/modules/mod-coa-content-scaling.conf".into(),
            b"[worldserver]\nCoAContentScaling.Enable = 1\n".to_vec(),
        );
        f.insert(
            "Settings/worldserver.conf.template".into(),
            b"[worldserver]\nRealmID = 1\n".to_vec(),
        );
        let w = wildcard_configs(f.clone()).unwrap();
        assert_eq!(
            w["Core/configs/modules/mod-coa-content-scaling.conf"],
            f["Core/configs/modules/mod-coa-content-scaling.conf"]
        );
        let c = ConfFile::parse_bytes(&w["Core/configs/modules/mod_coa_playerbots.conf"]).unwrap();
        assert_eq!(c.get("CoaBots.Enable"), Some("0"));
        assert_eq!(
            ConfFile::parse_bytes(&w["Settings/worldserver.conf.template"])
                .unwrap()
                .get("RealmID"),
            Some("2")
        );
    }

    #[test]
    fn corrupt_state_is_not_silently_treated_as_coa() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("Settings")).unwrap();
        fs::write(t.path().join(STATE), "broken").unwrap();
        assert!(state(t.path()).is_err());
    }

    #[test]
    fn recovery_restores_original_settings() {
        let t = tempfile::tempdir().unwrap();
        let mut files = Files::new();
        files.insert(
            "Core/configs/modules/coa.conf".into(),
            b"CoA.ClassModel = coa\n".to_vec(),
        );
        fsx::atomic_write_json(
            &t.path().join(JOURNAL),
            &Journal {
                before: RealmState::default(),
                files,
            },
        )
        .unwrap();
        recover(t.path()).unwrap();
        assert_eq!(state(t.path()).unwrap().active, Mode::Coa);
        assert_eq!(
            fs::read_to_string(t.path().join("Core/configs/modules/coa.conf")).unwrap(),
            "CoA.ClassModel = coa\n"
        );
        assert!(!t.path().join(JOURNAL).exists());
    }

    #[test]
    fn reverting_to_coa_removes_wildcard_only_config_files() {
        let t = tempfile::tempdir().unwrap();
        let mut coa = Files::new();
        coa.insert(
            "Core/configs/modules/coa.conf".into(),
            b"[worldserver]\nCoA.ClassModel = coa\n".to_vec(),
        );
        let wildcard = wildcard_configs(coa.clone()).unwrap();
        write_configs(t.path(), &wildcard).unwrap();
        write_configs(t.path(), &coa).unwrap();
        assert!(!t
            .path()
            .join("Core/configs/modules/mod-coa-challenges.conf")
            .exists());
        assert_eq!(configs(t.path()).unwrap(), coa);
    }

    #[test]
    fn wildcard_blocks_bot_settings_and_enable_calls() {
        let t = tempfile::tempdir().unwrap();
        fsx::atomic_write_json(
            &t.path().join(STATE),
            &RealmState {
                active: Mode::Wildcard,
                wildcard_created: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(crate::modules::set_enabled(t.path(), t.path(), "companions", true).is_err());
        assert!(crate::config::save(
            t.path(),
            t.path(),
            crate::config::Scope::Bots,
            &BTreeMap::new()
        )
        .is_err());
        assert!(guard_module(t.path(), "content-scaling").is_ok());
    }

    #[test]
    fn unknown_launcher_is_refused_without_partial_patching() {
        assert!(patch_launcher("print('custom launcher')").is_err());
    }

    #[test]
    fn managed_launcher_patch_remains_pristine_but_external_edits_conflict() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("server");
        let source = "(\"WorldDatabaseInfo\", \"acore_world\")\n(\"CharacterDatabaseInfo\", \"acore_characters\")\nSET name='AzerothCore',address=\nWHERE id=1;\nmysql(\"UPDATE acore_auth.realmlist SET flag=0\nif __name__ == \"__main__\":\n";
        fsx::atomic_write(&root.join("Scripts/manage.py"), source.as_bytes()).unwrap();
        let mut meta = crate::registry::InstallMeta::new(crate::registry::InstallKind::New, &root);
        meta.original_hashes.insert(
            "Scripts/manage.py".into(),
            fsx::sha256_bytes(source.as_bytes()),
        );
        let dir = crate::registry::MetaDir::create(&root, &meta).unwrap();
        prepare_launcher(&root).unwrap();
        let (_, meta) = crate::registry::MetaDir::open(&dir.root).unwrap();
        assert!(crate::diag::verify_managed(&root, &meta).is_empty());
        let manifest: crate::manifest::Manifest = serde_json::from_value(serde_json::json!({
            "schema": 1, "kind": "update", "version": "2.0.0", "core": {"commit": null}, "builtAt": "fixture", "minManagerVersion": "0.1.0",
            "files": [{"path":"Scripts/manage.py","sha256":fsx::sha256_bytes(b"next release"),"size":12,"owner":"core","policy":"replace"}]
        })).unwrap();
        assert_eq!(
            crate::update::plan(&root, &meta, &manifest, &BTreeMap::new(), None).unwrap()[0].action,
            crate::update::Action::Replace
        );
        let mut modified = fs::read(root.join("Scripts/manage.py")).unwrap();
        modified.extend_from_slice(b"# custom edit\n");
        fsx::atomic_write(&root.join("Scripts/manage.py"), &modified).unwrap();
        prepare_launcher(&root).unwrap();
        assert_eq!(
            crate::update::plan(&root, &meta, &manifest, &BTreeMap::new(), None).unwrap()[0].action,
            crate::update::Action::Conflict
        );
    }
}
