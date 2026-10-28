use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::parser::ConfFile;
use crate::error::{Error, Result};

pub const KEY: &str = "CoACustomRaces.DisabledRaceIds";
pub const CATALOG_KEY: &str = "CoACustomRaces.DisabledRaceCatalogSha256";
const CONF: &str = "Core/configs/modules/coa_custom_races.conf";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Race {
    pub id: u32,
    pub name: String,
    pub faction: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    pub realm: crate::realms::Mode,
    pub races: Vec<Race>,
    pub disabled_race_ids: BTreeSet<u32>,
    pub catalog_sha256: String,
    pub server_config_supported: bool,
    pub catalog_needs_review: bool,
    pub unrecognized_race_ids: BTreeSet<u32>,
}

fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}

fn stock(id: u32) -> bool {
    (1..=11).contains(&id) && id != 9
}

pub fn catalog(bytes: &[u8]) -> Result<Vec<Race>> {
    let word = |offset: usize| -> Result<u32> {
        let data = bytes
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("Truncated race catalog."))?;
        Ok(u32::from_le_bytes(data.try_into().unwrap()))
    };
    if bytes.get(..4) != Some(b"WDBC") || word(8)? != 69 || word(12)? != 276 {
        return Err(invalid("Unsupported race catalog format."));
    }
    let count = word(4)? as usize;
    let string_size = word(16)? as usize;
    if count == 0 || count > 255 {
        return Err(invalid("Invalid race catalog record count."));
    }
    let strings_start = 20 + count * 276;
    if strings_start.checked_add(string_size) != Some(bytes.len()) || string_size == 0 {
        return Err(invalid("Invalid race catalog bounds."));
    }
    let strings = &bytes[strings_start..];
    if strings.last() != Some(&0) {
        return Err(invalid("Invalid race catalog strings."));
    }
    let mut ids = BTreeSet::new();
    let mut races = Vec::new();
    for row in 0..count {
        let offset = 20 + row * 276;
        let id = word(offset)?;
        if id == 0 || id > 255 || !ids.insert(id) {
            return Err(invalid("Invalid or duplicate RaceID in the catalog."));
        }
        let mut name = String::new();
        for field in 14..30 {
            let index = word(offset + field * 4)? as usize;
            let remaining = strings
                .get(index..)
                .ok_or_else(|| invalid("Invalid race name offset."))?;
            let end = remaining
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| invalid("Unterminated race name."))?;
            let value = std::str::from_utf8(&remaining[..end])
                .map_err(|_| invalid("Invalid race name encoding."))?;
            if name.is_empty() && !value.is_empty() {
                name = value.into();
            }
        }
        if stock(id) || word(offset + 4)? & 1 != 0 {
            continue;
        }
        if name.trim().is_empty() {
            return Err(invalid("A playable custom race has no name."));
        }
        let faction = match word(offset + 13 * 4)? {
            0 => Some("alliance".into()),
            1 => Some("horde".into()),
            _ => None,
        };
        races.push(Race { id, name, faction });
    }
    races.sort_by_key(|race| race.id);
    Ok(races)
}

fn parse_ids(value: &str) -> Result<BTreeSet<u32>> {
    let value = value.trim();
    let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        &value[1..value.len() - 1]
    } else {
        value
    };
    if value.trim().is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut disabled = BTreeSet::new();
    for token in value.split(',') {
        let token = token.trim();
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid(
                "Disabled RaceIDs must be a comma-separated list of whole numbers.",
            ));
        }
        let id = token
            .parse::<u32>()
            .map_err(|_| invalid("Invalid disabled RaceID."))?;
        if id == 0 || id > 255 || stock(id) || !disabled.insert(id) {
            return Err(invalid(
                "Disabled RaceIDs contain a stock, unknown or duplicate race.",
            ));
        }
    }
    Ok(disabled)
}

pub fn parse_disabled(value: &str, races: &[Race]) -> Result<BTreeSet<u32>> {
    let disabled = parse_ids(value)?;
    let known: BTreeSet<u32> = races.iter().map(|race| race.id).collect();
    if !disabled.is_subset(&known) {
        return Err(invalid("Disabled RaceIDs contain an unknown race."));
    }
    Ok(disabled)
}

fn dbc_path(root: &Path) -> Result<PathBuf> {
    let config_path = root.join("Core/configs/worldserver.conf");
    if config_path.is_file() {
        let conf = ConfFile::parse_bytes(&fs::read(config_path)?)?;
        let value = conf.get("DataDir").unwrap_or(".");
        let data = PathBuf::from(value.trim().trim_matches('"'));
        return Ok(if data.is_absolute() {
            data
        } else {
            root.join("Core").join(data)
        }
        .join("dbc_races/ChrRaces.dbc"));
    }
    Ok(root.join("Data/dbc_races/ChrRaces.dbc"))
}

pub fn status(root: &Path) -> Result<Availability> {
    let bytes = fs::read(dbc_path(root)?)?;
    let races = catalog(&bytes)?;
    let conf = ConfFile::parse_bytes(&fs::read(root.join(CONF))?)?;
    let configured = parse_ids(conf.get(KEY).unwrap_or(""))?;
    let known: BTreeSet<u32> = races.iter().map(|race| race.id).collect();
    let unrecognized_race_ids: BTreeSet<u32> = configured.difference(&known).copied().collect();
    let disabled_race_ids = configured.intersection(&known).copied().collect();
    let dist = fs::read(root.join(format!("{CONF}.dist")))
        .ok()
        .map(|bytes| ConfFile::parse_bytes(&bytes))
        .transpose()?;
    let catalog_sha256 = hex::encode(Sha256::digest(&bytes));
    let saved_hash = conf
        .get(CATALOG_KEY)
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase();
    let catalog_needs_review = !configured.is_empty()
        && (saved_hash != catalog_sha256 || !unrecognized_race_ids.is_empty());
    Ok(Availability {
        realm: crate::realms::state(root)?.active,
        races,
        disabled_race_ids,
        catalog_sha256,
        catalog_needs_review,
        unrecognized_race_ids,
        server_config_supported: dist
            .is_some_and(|file| file.get(KEY).is_some() && file.get(CATALOG_KEY).is_some())
            && fs::read(crate::layout::executables(root).0).is_ok_and(|binary| {
                [KEY, CATALOG_KEY]
                    .iter()
                    .all(|key| binary.windows(key.len()).any(|part| part == key.as_bytes()))
            }),
    })
}

pub fn save(
    root: &Path,
    meta: &Path,
    expected_catalog: &str,
    expected_realm: crate::realms::Mode,
    disabled: &[u32],
) -> Result<Availability> {
    crate::update::ensure_recovered(meta)?;
    fs::create_dir_all(root.join(".state"))?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(".state/control.lock"))?;
    if !fs4::fs_std::FileExt::try_lock_exclusive(&lock)? {
        return Err(invalid("Another server operation is in progress."));
    }
    let observed = crate::process::observe(root, &crate::layout::read_ports(root));
    if observed.world.state != crate::process::ServiceState::Stopped
        || crate::multiworld::is_running(root)
    {
        return Err(invalid(
            "Stop the server before changing race availability.",
        ));
    }
    let current = status(root)?;
    if current.realm != expected_realm {
        return Err(invalid(
            "The selected realm changed. Reload race availability before saving.",
        ));
    }
    if !current.server_config_supported {
        return Err(invalid(
            "Update the server to a build supporting individual race restrictions first.",
        ));
    }
    if current.catalog_sha256 != expected_catalog {
        return Err(invalid(
            "The race catalog changed. Reload the race list before saving.",
        ));
    }
    let value = disabled
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let normalized = parse_disabled(&value, &current.races)?;
    let value = normalized
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    crate::modules::save_settings(
        root,
        meta,
        "custom-races",
        &BTreeMap::from([
            (KEY.into(), format!("\"{value}\"")),
            (
                CATALOG_KEY.into(),
                format!(
                    "\"{}\"",
                    if normalized.is_empty() {
                        ""
                    } else {
                        &current.catalog_sha256
                    }
                ),
            ),
        ]),
    )?;
    status(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dbc() -> Vec<u8> {
        let mut bytes = b"WDBC".to_vec();
        for value in [4u32, 69, 276, 11] {
            bytes.extend(value.to_le_bytes());
        }
        for id in [1u32, 82, 83, 97] {
            let mut row = vec![0; 276];
            row[..4].copy_from_slice(&id.to_le_bytes());
            row[4..8].copy_from_slice(&12u32.to_le_bytes());
            row[56..60].copy_from_slice(&1u32.to_le_bytes());
            bytes.extend(row);
        }
        bytes.extend(b"\0Synthetic\0");
        bytes
    }

    #[test]
    fn catalog_preserves_ids_and_excludes_stock_and_nonplayable() {
        let mut bytes = dbc();
        bytes[20 + 276 + 4] |= 1;
        let races = catalog(&bytes).unwrap();
        assert_eq!(
            races.iter().map(|race| race.id).collect::<Vec<_>>(),
            [83, 97]
        );
        assert_eq!(races[0].name, "Synthetic");
    }

    #[test]
    fn rejects_corrupt_layout_bounds_ids_and_names() {
        for (offset, value) in [
            (4, 0),
            (8, 68),
            (12, 272),
            (16, 100),
            (20, 0),
            (20 + 276, 1),
            (20 + 56, 999),
        ] {
            let mut bytes = dbc();
            bytes[offset..offset + 4].copy_from_slice(&(value as u32).to_le_bytes());
            assert!(catalog(&bytes).is_err(), "offset {offset}");
        }
        assert!(catalog(&dbc()[..19]).is_err());
        assert!(catalog(&dbc()[..100]).is_err());
    }

    #[test]
    fn disabled_set_is_strict_and_atomic() {
        let races = catalog(&dbc()).unwrap();
        assert_eq!(
            parse_disabled("\"97,82\"", &races).unwrap(),
            BTreeSet::from([82, 97])
        );
        assert!(parse_disabled("\"\"", &races).unwrap().is_empty());
        for value in [
            "82,1", "82,82", "82,250", "82,", "-82", "+82", "82,1-97", "82x", "0",
        ] {
            assert!(parse_disabled(value, &races).is_err(), "{value}");
        }
    }

    #[test]
    fn status_uses_current_config_and_requires_server_capability() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir_all(root.join("Data/dbc_races")).unwrap();
        fs::create_dir_all(root.join("Core/configs/modules")).unwrap();
        fs::write(root.join("Data/dbc_races/ChrRaces.dbc"), dbc()).unwrap();
        fs::write(root.join(CONF), format!("{KEY} = \"82\"\n")).unwrap();
        let before = status(root).unwrap();
        assert!(!before.server_config_supported);
        assert_eq!(before.disabled_race_ids, BTreeSet::from([82]));
        fs::write(
            root.join(format!("{CONF}.dist")),
            format!("{KEY} = \"\"\n{CATALOG_KEY} = \"\"\n"),
        )
        .unwrap();
        assert!(!status(root).unwrap().server_config_supported);
        fs::write(
            root.join("Core/worldserver.exe"),
            format!("{KEY} {CATALOG_KEY}"),
        )
        .unwrap();
        let after = status(root).unwrap();
        assert!(after.server_config_supported);
        assert_eq!(before.catalog_sha256, after.catalog_sha256);
    }

    fn server(root: &Path) {
        fs::create_dir_all(root.join("Data/dbc_races")).unwrap();
        fs::create_dir_all(root.join("Core/configs/modules")).unwrap();
        fs::write(
            root.join("Core/worldserver.exe"),
            format!("{KEY} {CATALOG_KEY}"),
        )
        .unwrap();
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::write(
            root.join("Settings/repack.json"),
            r#"{"worldPort":0,"authPort":0,"mysqlPort":0,"raPort":0}"#,
        )
        .unwrap();
        fs::write(root.join("Data/dbc_races/ChrRaces.dbc"), dbc()).unwrap();
        fs::write(
            root.join(CONF),
            format!("CoACustomRaces.Enable = 1\n{KEY} = \"82\"\nUser.Custom = 42\n"),
        )
        .unwrap();
        fs::write(
            root.join(format!("{CONF}.dist")),
            format!("CoACustomRaces.Enable = 0\n{KEY} = \"\"\n{CATALOG_KEY} = \"\"\n"),
        )
        .unwrap();
        fs::write(
            root.join("Core/configs/modules/playerbots.conf"),
            "AiPlayerbot.ExcludedBotRaces = \"1,83\"\nAiPlayerbot.CrashingBotRaces = \"97\"\n",
        )
        .unwrap();
    }

    #[test]
    fn saving_and_allow_all_preserve_user_settings_and_bot_exclusions() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("server");
        let meta = temporary.path().join("metadata");
        server(&root);
        let hash = status(&root).unwrap().catalog_sha256;
        let bot_path = root.join("Core/configs/modules/playerbots.conf");
        let bot_before = fs::read(&bot_path).unwrap();
        let saved = save(&root, &meta, &hash, crate::realms::Mode::Coa, &[97, 83]).unwrap();
        assert_eq!(saved.disabled_race_ids, BTreeSet::from([83, 97]));
        assert!(!saved.catalog_needs_review);
        assert!(fs::read_to_string(root.join(CONF)).unwrap().contains(&hash));
        assert!(fs::read_to_string(root.join(CONF))
            .unwrap()
            .contains("User.Custom = 42"));
        assert_eq!(fs::read(&bot_path).unwrap(), bot_before);
        assert!(save(&root, &meta, &hash, crate::realms::Mode::Coa, &[])
            .unwrap()
            .disabled_race_ids
            .is_empty());
    }

    #[test]
    fn stale_catalog_and_invalid_lists_cannot_change_configuration() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("server");
        let meta = temporary.path().join("metadata");
        server(&root);
        let hash = status(&root).unwrap().catalog_sha256;
        let before = fs::read(root.join(CONF)).unwrap();
        assert!(save(&root, &meta, "stale", crate::realms::Mode::Coa, &[83]).is_err());
        for ids in [&[82, 1][..], &[82, 82][..], &[250][..]] {
            assert!(save(&root, &meta, &hash, crate::realms::Mode::Coa, ids).is_err());
            assert_eq!(fs::read(root.join(CONF)).unwrap(), before);
        }
    }

    #[test]
    fn switching_realms_cannot_apply_a_draft_from_another_profile() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("server");
        let meta = temporary.path().join("metadata");
        server(&root);
        let loaded = status(&root).unwrap();
        let before = fs::read(root.join(CONF)).unwrap();
        let realm = crate::realms::RealmState {
            active: crate::realms::Mode::Wildcard,
            ..Default::default()
        };
        fs::write(
            root.join("Settings/realm-profile.json"),
            serde_json::to_vec(&realm).unwrap(),
        )
        .unwrap();
        assert!(save(&root, &meta, &loaded.catalog_sha256, loaded.realm, &[83]).is_err());
        assert_eq!(fs::read(root.join(CONF)).unwrap(), before);
    }

    #[test]
    fn concurrent_server_operations_cannot_change_policy() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("server");
        let meta = temporary.path().join("metadata");
        server(&root);
        fs::create_dir_all(root.join(".state")).unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".state/control.lock"))
            .unwrap();
        assert!(fs4::fs_std::FileExt::try_lock_exclusive(&lock).unwrap());
        let loaded = status(&root).unwrap();
        let before = fs::read(root.join(CONF)).unwrap();
        assert!(save(&root, &meta, &loaded.catalog_sha256, loaded.realm, &[83]).is_err());
        assert_eq!(fs::read(root.join(CONF)).unwrap(), before);
    }

    #[test]
    fn relative_data_dir_uses_the_worldserver_working_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("server");
        server(&root);
        fs::write(
            root.join("Core/configs/worldserver.conf"),
            "DataDir = \"../Data\"\n",
        )
        .unwrap();
        assert_eq!(status(&root).unwrap().races.len(), 3);
    }

    #[test]
    fn changed_catalog_requires_explicit_review_and_unknown_ids_are_not_valid_races() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("server");
        let meta = temporary.path().join("metadata");
        server(&root);
        let loaded = status(&root).unwrap();
        assert!(loaded.catalog_needs_review);
        save(&root, &meta, &loaded.catalog_sha256, loaded.realm, &[82]).unwrap();
        assert!(!status(&root).unwrap().catalog_needs_review);
        let mut changed = dbc();
        changed[20 + 276..24 + 276].copy_from_slice(&84u32.to_le_bytes());
        fs::write(root.join("Data/dbc_races/ChrRaces.dbc"), changed).unwrap();
        let updated = status(&root).unwrap();
        assert!(updated.catalog_needs_review);
        assert_eq!(updated.unrecognized_race_ids, BTreeSet::from([82]));
        assert!(!updated.races.iter().any(|race| race.id == 82));
        assert!(updated.disabled_race_ids.is_empty());
        let reviewed = save(&root, &meta, &updated.catalog_sha256, updated.realm, &[]).unwrap();
        assert!(!reviewed.catalog_needs_review);
        assert!(reviewed.unrecognized_race_ids.is_empty());
    }
}
