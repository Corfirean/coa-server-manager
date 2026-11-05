use crate::{config::parser::ConfFile, fsx, Error, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

const FILES: &[&str] = &[
    "Settings/repack.json",
    "Settings/realm-profile.json",
    "Settings/worldserver.conf.template",
    "Settings/authserver.conf.template",
    "Core/configs/worldserver.conf",
    "Core/configs/authserver.conf",
    "Settings/realm-profiles/coa.json",
    "Settings/realm-profiles/wildcard.json",
    ".realms/secondary/Settings/repack.json",
    ".realms/secondary/Settings/realm-profile.json",
    ".realms/secondary/Settings/worldserver.conf.template",
    ".realms/secondary/Settings/authserver.conf.template",
    ".realms/secondary/Core/configs/worldserver.conf",
    ".realms/secondary/Core/configs/authserver.conf",
    "Settings/realm-profiles/reborn.json",
    ".realms/reborn/Settings/repack.json",
    ".realms/reborn/Settings/realm-profile.json",
    ".realms/reborn/Settings/worldserver.conf.template",
    ".realms/reborn/Settings/authserver.conf.template",
    ".realms/reborn/Core/configs/worldserver.conf",
    ".realms/reborn/Core/configs/authserver.conf",
];

/// Ports of the extra worlds kept in the realm profile.
const REALM_PORT_KEYS: [&str; 4] = [
    "secondary_world_port",
    "secondary_ra_port",
    "reborn_world_port",
    "reborn_ra_port",
];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    bytes: Option<Vec<u8>>,
    sha256: Option<String>,
}

fn bot_files() -> Vec<String> {
    ["", ".realms/secondary/", ".realms/reborn/"]
        .into_iter()
        .flat_map(|prefix| {
            [
                "mod_coa_playerbots.conf",
                "mod_coa_playerbots.conf.dist",
                "playerbots.conf",
                "playerbots.conf.dist",
                "mod_playerbots.conf",
                "mod_playerbots.conf.dist",
            ]
            .into_iter()
            .map(move |name| format!("{prefix}Core/configs/modules/{name}"))
        })
        .collect()
}

pub(crate) fn pending(meta: &Path) -> bool {
    meta.join("validation-configs.json").exists()
}

pub(crate) fn begin(root: &Path, meta: &Path) -> Result<()> {
    if pending(meta) {
        return Err(Error::Invalid(
            "Interrupted isolated validation must be recovered first.".into(),
        ));
    }
    let mut saved = BTreeMap::new();
    let modules = bot_files();
    for rel in FILES
        .iter()
        .map(|rel| rel.to_string())
        .chain(modules.iter().cloned())
    {
        let path = fsx::ensure_within(root, &fsx::safe_join(root, &rel)?)?;
        let bytes = if path.exists() {
            Some(fs::read(path)?)
        } else {
            None
        };
        let sha256 = bytes.as_ref().map(|b| fsx::sha256_bytes(b));
        saved.insert(rel, Saved { bytes, sha256 });
    }
    let mut settings: serde_json::Value = fsx::read_json(&root.join("Settings/repack.json"))?;
    let mut realm: Option<serde_json::Value> = if root.join("Settings/realm-profile.json").exists()
    {
        Some(fsx::read_json(&root.join("Settings/realm-profile.json"))?)
    } else {
        None
    };
    let mut excluded: std::collections::BTreeSet<u64> =
        ["mysqlPort", "authPort", "worldPort", "raPort"]
            .iter()
            .filter_map(|key| settings[*key].as_u64())
            .collect();
    if let Some(state) = &realm {
        excluded.extend(
            REALM_PORT_KEYS
                .iter()
                .filter_map(|key| state[*key].as_u64()),
        );
    }
    let mut sockets = Vec::new();
    let mut reserve = || -> Result<u16> {
        loop {
            let socket = std::net::TcpListener::bind("127.0.0.1:0")?;
            let port = socket.local_addr()?.port();
            if excluded.insert(u64::from(port)) {
                sockets.push(socket);
                return Ok(port);
            }
        }
    };
    for key in ["authPort", "worldPort", "raPort"] {
        settings[key] = reserve()?.into();
    }
    if let Some(state) = &mut realm {
        for key in REALM_PORT_KEYS {
            state[key] = reserve()?.into();
        }
    }
    let mut configs = Vec::new();
    for rel in FILES.iter().copied().filter(|rel| {
        rel.ends_with("worldserver.conf.template")
            || rel.ends_with("authserver.conf.template")
            || rel.ends_with("worldserver.conf")
            || rel.ends_with("authserver.conf")
    }) {
        let path = root.join(rel);
        if !path.exists() && rel.ends_with(".template") && !rel.starts_with(".realms/") {
            return Err(Error::Invalid(format!(
                "Cannot isolate validation without {rel}."
            )));
        }
        if path.exists() {
            let mut conf = ConfFile::parse_bytes(&fs::read(&path)?)?;
            conf.set("BindIP", "\"127.0.0.1\"", &[]);
            if rel.contains("worldserver") {
                conf.set("Ra.IP", "\"127.0.0.1\"", &[]);
                conf.set("CoA.ValidationMode", "1", &[]);
            }
            configs.push((path, conf.to_text()));
        }
    }
    for rel in modules {
        let path = fsx::ensure_within(root, &fsx::safe_join(root, &rel)?)?;
        if !path.exists() {
            continue;
        }
        let mut conf = ConfFile::parse_bytes(&fs::read(&path)?)?;
        let keys: &[&str] = if rel.contains("mod_coa_playerbots.") {
            &[
                "CoaBots.AutoLoginOnStartup",
                "CoaBots.RandomSpawn.AutoLogin",
            ]
        } else {
            &[
                "AiPlayerbot.RandomBotAutologin",
                "AiPlayerbot.BotAutologin",
                "AiPlayerbot.DeleteRandomBotAccounts",
            ]
        };
        for key in keys {
            conf.set(key, "0", &[]);
        }
        configs.push((path, conf.to_text()));
    }
    fsx::atomic_write_json(&meta.join("validation-configs.json"), &saved)?;
    fsx::atomic_write_json(&root.join("Settings/repack.json"), &settings)?;
    if let Some(state) = realm {
        fsx::atomic_write_json(&root.join("Settings/realm-profile.json"), &state)?;
    }
    for (path, text) in configs {
        fsx::atomic_write(&path, text.as_bytes())?;
    }
    Ok(())
}

pub(crate) fn restore(root: &Path, meta: &Path) -> Result<()> {
    if !pending(meta) {
        return Ok(());
    }
    let journal = meta.join("validation-configs.json");
    let saved: BTreeMap<String, Saved> = fsx::read_json(&journal)?;
    let modules = bot_files();
    let complete_core = FILES.iter().all(|rel| saved.contains_key(*rel));
    let legacy_core = FILES
        .iter()
        .filter(|rel| **rel != "Settings/realm-profiles/reborn.json")
        .all(|rel| saved.contains_key(*rel));
    let current = complete_core
        && saved.len() == FILES.len() + modules.len()
        && modules.iter().all(|rel| saved.contains_key(rel));
    let previous = complete_core && saved.len() == FILES.len();
    let legacy = legacy_core
        && saved.len() == FILES.len() - 1
        && !saved.contains_key("Settings/realm-profiles/reborn.json");
    if !(current || previous || legacy) {
        return Err(Error::Invalid(
            "Isolated validation configuration journal is incomplete.".into(),
        ));
    }
    for (rel, file) in &saved {
        fsx::ensure_within(root, &fsx::safe_join(root, rel)?)?;
        if file.bytes.as_ref().map(|b| fsx::sha256_bytes(b)) != file.sha256 {
            return Err(Error::Invalid(format!(
                "Isolated validation recovery copy {rel} is damaged."
            )));
        }
    }
    for (rel, file) in saved {
        let path = fsx::ensure_within(root, &fsx::safe_join(root, &rel)?)?;
        match file.bytes {
            Some(bytes) => fsx::atomic_write(&path, &bytes)?,
            None if path.exists() => fs::remove_file(path)?,
            None => {}
        }
    }
    fs::remove_file(journal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("server");
        let meta = dir.path().join("metadata");
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::create_dir_all(&meta).unwrap();
        fsx::atomic_write_json(
            &root.join(FILES[0]),
            &serde_json::json!({"mysqlPort":3306,"authPort":3724,"worldPort":8085,"raPort":3443}),
        )
        .unwrap();
        fsx::atomic_write_json(&root.join(FILES[1]), &serde_json::json!({"simultaneous":true,"secondary_world_port":8086,"secondary_ra_port":3444})).unwrap();
        for rel in [FILES[2], FILES[3]] {
            fsx::atomic_write(
                &root.join(rel),
                b"BindIP = \"0.0.0.0\"\r\nRa.IP = \"0.0.0.0\"\r\n",
            )
            .unwrap();
        }
        (dir, root, meta)
    }

    #[test]
    fn interrupted_validation_restores_original_bytes_and_absent_files() {
        let (_dir, root, meta) = fixture();
        fsx::atomic_write(
            &root.join("Settings/realm-profiles/reborn.json"),
            b"saved reborn profile",
        )
        .unwrap();
        let original: Vec<_> = FILES.iter().map(|p| fs::read(root.join(p)).ok()).collect();
        begin(&root, &meta).unwrap();
        assert!(crate::update::ensure_recovered(&meta).is_err());
        assert!(begin(&root, &meta).is_err());
        let settings: serde_json::Value = fsx::read_json(&root.join(FILES[0])).unwrap();
        assert_eq!(settings["mysqlPort"], 3306);
        let realm: serde_json::Value = fsx::read_json(&root.join(FILES[1])).unwrap();
        let ports: std::collections::BTreeSet<_> = [
            settings["authPort"].as_u64(),
            settings["worldPort"].as_u64(),
            settings["raPort"].as_u64(),
            realm["secondary_world_port"].as_u64(),
            realm["secondary_ra_port"].as_u64(),
        ]
        .into_iter()
        .collect();
        assert_eq!(ports.len(), 5);
        let config = ConfFile::parse_bytes(&fs::read(root.join(FILES[2])).unwrap()).unwrap();
        assert_eq!(config.get("BindIP"), Some("\"127.0.0.1\""));
        fsx::atomic_write(&root.join(FILES[4]), b"rendered temporary configuration").unwrap();
        fsx::atomic_write(
            &root.join("Settings/realm-profiles/reborn.json"),
            b"temporary reborn profile",
        )
        .unwrap();
        restore(&root, &meta).unwrap();
        for (rel, bytes) in FILES.iter().zip(original) {
            assert_eq!(fs::read(root.join(rel)).ok(), bytes);
        }
        assert!(!pending(&meta));
        restore(&root, &meta).unwrap();
    }

    #[test]
    fn damaged_recovery_copy_blocks_all_restoration() {
        let (_dir, root, meta) = fixture();
        begin(&root, &meta).unwrap();
        let journal = meta.join("validation-configs.json");
        let mut saved: BTreeMap<String, Saved> = fsx::read_json(&journal).unwrap();
        saved.get_mut(FILES[0]).unwrap().bytes = Some(b"damaged".to_vec());
        fsx::atomic_write_json(&journal, &saved).unwrap();
        let before = fs::read(root.join(FILES[2])).unwrap();
        assert!(restore(&root, &meta).is_err());
        assert_eq!(fs::read(root.join(FILES[2])).unwrap(), before);
        assert!(pending(&meta));
    }

    #[test]
    fn missing_template_fails_before_any_configuration_changes() {
        let (_dir, root, meta) = fixture();
        fs::remove_file(root.join(FILES[3])).unwrap();
        let before = fs::read(root.join(FILES[0])).unwrap();
        assert!(begin(&root, &meta).is_err());
        assert!(!pending(&meta));
        assert_eq!(fs::read(root.join(FILES[0])).unwrap(), before);
    }

    #[test]
    fn bot_population_is_suppressed_in_all_realms_and_restored_exactly() {
        let (_dir, root, meta) = fixture();
        let companion = b"[worldserver]\r\n# owner settings\r\nCoaBots.Enable = 1\r\nCoaBots.AutoLoginOnStartup = 1\r\nCoaBots.RandomSpawn.AutoLogin = 1\r\nCoaBots.AutoLogin.MaxCount = 2000\r\n";
        let squid = b"[worldserver]\nAiPlayerbot.Enabled = 1\nAiPlayerbot.RandomBotAutologin = 1\nAiPlayerbot.BotAutologin = 1\nAiPlayerbot.DeleteRandomBotAccounts = 1\n";
        for rel in bot_files() {
            fsx::atomic_write(
                &root.join(&rel),
                if rel.contains("mod_coa_playerbots.") {
                    companion
                } else {
                    squid
                },
            )
            .unwrap();
        }
        begin(&root, &meta).unwrap();
        for rel in bot_files() {
            let conf = ConfFile::parse_bytes(&fs::read(root.join(&rel)).unwrap()).unwrap();
            if rel.contains("mod_coa_playerbots.") {
                assert_eq!(conf.get("CoaBots.Enable"), Some("1"));
                assert_eq!(conf.get("CoaBots.AutoLoginOnStartup"), Some("0"));
                assert_eq!(conf.get("CoaBots.RandomSpawn.AutoLogin"), Some("0"));
                assert_eq!(conf.get("CoaBots.AutoLogin.MaxCount"), Some("2000"));
            } else {
                assert_eq!(conf.get("AiPlayerbot.Enabled"), Some("1"));
                assert_eq!(conf.get("AiPlayerbot.RandomBotAutologin"), Some("0"));
                assert_eq!(conf.get("AiPlayerbot.BotAutologin"), Some("0"));
                assert_eq!(conf.get("AiPlayerbot.DeleteRandomBotAccounts"), Some("0"));
            }
        }
        restore(&root, &meta).unwrap();
        for rel in bot_files() {
            assert_eq!(
                fs::read(root.join(&rel)).unwrap(),
                if rel.contains("mod_coa_playerbots.") {
                    companion.as_slice()
                } else {
                    squid.as_slice()
                }
            );
        }
    }

    #[test]
    fn validation_removes_only_module_configs_created_during_the_check() {
        let (_dir, root, meta) = fixture();
        let dist = root.join("Core/configs/modules/mod_coa_playerbots.conf.dist");
        let active = root.join("Core/configs/modules/mod_coa_playerbots.conf");
        let original = b"CoaBots.AutoLoginOnStartup = 1\n";
        fsx::atomic_write(&dist, original).unwrap();
        begin(&root, &meta).unwrap();
        fs::copy(&dist, &active).unwrap();
        restore(&root, &meta).unwrap();
        assert!(!active.exists());
        assert_eq!(fs::read(dist).unwrap(), original);
    }

    #[test]
    fn previous_manager_journals_remain_recoverable() {
        for without_reborn in [false, true] {
            let (_dir, root, meta) = fixture();
            begin(&root, &meta).unwrap();
            let journal = meta.join("validation-configs.json");
            let mut saved: BTreeMap<String, Saved> = fsx::read_json(&journal).unwrap();
            saved.retain(|key, _| FILES.contains(&key.as_str()));
            if without_reborn {
                saved.remove("Settings/realm-profiles/reborn.json");
            }
            fsx::atomic_write_json(&journal, &saved).unwrap();
            restore(&root, &meta).unwrap();
            assert!(!pending(&meta));
        }
    }

    #[test]
    fn incomplete_bot_journal_blocks_restoration_before_writing() {
        let (_dir, root, meta) = fixture();
        begin(&root, &meta).unwrap();
        let journal = meta.join("validation-configs.json");
        let mut saved: BTreeMap<String, Saved> = fsx::read_json(&journal).unwrap();
        saved.remove(&bot_files()[0]);
        fsx::atomic_write_json(&journal, &saved).unwrap();
        let before = fs::read(root.join(FILES[2])).unwrap();
        assert!(restore(&root, &meta).is_err());
        assert_eq!(fs::read(root.join(FILES[2])).unwrap(), before);
    }
}
