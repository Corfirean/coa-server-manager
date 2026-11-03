use std::{fs, path::Path};

use crate::{
    config::{parser::ConfFile, Scope},
    error::{Error, Result},
    fsx,
};

fn transform(bytes: &[u8], preset: &str) -> Result<String> {
    let level = match preset {
        "normal" => 4,
        "errors" => 2,
        "off" => 0,
        _ => return Err(Error::Invalid("Unknown server logging preset.".into())),
    };
    let mut conf = ConfFile::parse_bytes(bytes)?;
    let entries: Vec<_> = conf
        .entries()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    for (key, value) in entries {
        if key.starts_with("Logger.") {
            let Some((_, appenders)) = value.split_once(',') else {
                continue;
            };
            let value = if preset == "normal"
                && matches!(
                    key.as_str(),
                    "Logger.network" | "Logger.sql.sql" | "Logger.entities.player"
                ) {
                2
            } else {
                level
            };
            conf.set(&key, &format!("{value},{appenders}"), &[]);
        } else if key.starts_with("Appender.") {
            let mut fields: Vec<_> = value.split(',').map(str::to_string).collect();
            if fields.len() >= 3 {
                fields[1] = if key == "Appender.Errors" && preset == "normal" {
                    "2".into()
                } else {
                    level.to_string()
                };
                conf.set(&key, &fields.join(","), &[]);
            }
        }
    }
    Ok(conf.to_text())
}

pub fn apply(root: &Path, meta: &Path, preset: &str) -> Result<()> {
    let _operation = crate::update::operation_lock(meta)?;
    crate::update::ensure_recovered(meta)?;
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
    let mut originals = Vec::new();
    let mut texts = Vec::new();
    for prefix in ["", ".realms/secondary/", ".realms/reborn/"] {
        for name in [
            "Settings/worldserver.conf.template",
            "Settings/authserver.conf.template",
            "Core/configs/worldserver.conf",
            "Core/configs/authserver.conf",
        ] {
            let path = fsx::ensure_within(root, &root.join(format!("{prefix}{name}")))?;
            if path.is_file() {
                let bytes = fs::read(&path)?;
                texts.push(transform(&bytes, preset)?);
                originals.push((path, bytes));
            }
        }
        let marker = root.join(format!("{prefix}.state/debug-logs.json"));
        if marker.is_file() {
            originals.push((marker.clone(), fs::read(marker)?));
            texts.push("{\"enabled\":false}\n".into());
        }
    }
    if originals.is_empty() {
        return Err(Error::Invalid(
            "Server configuration files were not found.".into(),
        ));
    }
    crate::config::take_snapshot(
        meta,
        Scope::Server,
        "before changing server logging",
        &originals,
    )?;
    for (index, (path, _)) in originals.iter().enumerate() {
        if let Err(error) = fsx::atomic_write(path, texts[index].as_bytes()) {
            for (path, bytes) in originals.iter().take(index) {
                let _ = fsx::atomic_write(path, bytes);
            }
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_auth_world_and_templates_in_all_realms_with_restorable_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("server");
        let meta = temp.path().join("meta");
        fs::create_dir_all(&root).unwrap();
        let mut files = Vec::new();
        for prefix in ["", ".realms/secondary/", ".realms/reborn/"] {
            for name in [
                "Settings/worldserver.conf.template",
                "Settings/authserver.conf.template",
                "Core/configs/worldserver.conf",
                "Core/configs/authserver.conf",
            ] {
                let path = root.join(format!("{prefix}{name}"));
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                let bytes =
                    b"Logger.root=6,Console\r\nAppender.Console=1,6,0\r\nKeep = unchanged\r\n"
                        .to_vec();
                fs::write(&path, &bytes).unwrap();
                files.push((path, bytes));
            }
        }
        apply(&root, &meta, "errors").unwrap();
        for (path, _) in &files {
            assert!(fs::read_to_string(path)
                .unwrap()
                .contains("Logger.root=2,Console\r\n"));
        }
        let snapshots = crate::config::list_snapshots(&meta);
        assert_eq!(snapshots.len(), 1);
        crate::config::restore_snapshot(&meta, &snapshots[0].id).unwrap();
        for (path, bytes) in files {
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }

    #[test]
    fn presets_control_dispatch_and_appenders_and_can_restore_useful_logs() {
        let original = b"Logger.root=6,Console Server\r\nLogger.sql.sql=6,Console Server\r\nAppender.Console=1,6,0,\"1 9 3 6 5 8\"\r\nAppender.Errors=2,2,0,Errors.log,w\r\nOther = keep\r\n";
        let off = transform(original, "off").unwrap();
        assert!(off.contains("Logger.root=0,Console Server\r\n"));
        assert!(off.contains("Appender.Errors=2,0,0,Errors.log,w\r\n"));
        let normal = transform(off.as_bytes(), "normal").unwrap();
        assert!(normal.contains("Logger.root=4,Console Server\r\n"));
        assert!(normal.contains("Logger.sql.sql=2,Console Server\r\n"));
        assert!(normal.contains("Appender.Errors=2,2,0,Errors.log,w\r\n"));
        assert!(normal.contains("Other = keep\r\n"));
        assert_eq!(transform(normal.as_bytes(), "normal").unwrap(), normal);
        assert!(transform(original, "unsupported").is_err());
    }
}
