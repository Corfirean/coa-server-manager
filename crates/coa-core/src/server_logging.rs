use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    config::{parser::ConfFile, Scope},
    error::{Error, Result},
    fsx,
};

const SETTINGS_FILE: &str = "Settings/log-settings.json";
const DEFAULT_MAX_SIZE_MB: u64 = 500;
const MIN_MAX_SIZE_MB: u64 = 10;
const MAX_MAX_SIZE_MB: u64 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LogSettings {
    pub auto_cleanup: bool,
    pub max_size_mb: u64,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            auto_cleanup: false,
            max_size_mb: DEFAULT_MAX_SIZE_MB,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanLogsReport {
    pub freed_bytes: u64,
    pub remaining_bytes: u64,
    pub deleted_files: u64,
    pub skipped_files: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerLoggingView {
    pub preset: Option<String>,
    pub log_bytes: u64,
    pub settings: LogSettings,
}

fn owner(root: &Path) -> PathBuf {
    if crate::launcher::settings::is_extra_world(root) {
        if let Some(main) = root.parent().and_then(Path::parent) {
            return main.to_path_buf();
        }
    }
    root.to_path_buf()
}

pub fn load_settings(root: &Path) -> LogSettings {
    let mut settings: LogSettings =
        fsx::read_json(&owner(root).join(SETTINGS_FILE)).unwrap_or_default();
    if settings.max_size_mb < MIN_MAX_SIZE_MB || settings.max_size_mb > MAX_MAX_SIZE_MB {
        settings.max_size_mb = DEFAULT_MAX_SIZE_MB;
    }
    settings
}

pub fn save_settings(root: &Path, settings: &LogSettings) -> Result<LogSettings> {
    let mut settings = settings.clone();
    if settings.auto_cleanup {
        if !(MIN_MAX_SIZE_MB..=MAX_MAX_SIZE_MB).contains(&settings.max_size_mb) {
            return Err(Error::Invalid(format!(
                "Log auto-cleanup threshold must be between {MIN_MAX_SIZE_MB} and {MAX_MAX_SIZE_MB} MB."
            )));
        }
    } else if !(MIN_MAX_SIZE_MB..=MAX_MAX_SIZE_MB).contains(&settings.max_size_mb) {
        settings.max_size_mb = DEFAULT_MAX_SIZE_MB;
    }
    fsx::atomic_write_json(&owner(root).join(SETTINGS_FILE), &settings)?;
    Ok(settings)
}

pub fn detect_preset(root: &Path) -> Option<String> {
    let candidates = [
        root.join("Core/configs/worldserver.conf"),
        root.join("Settings/worldserver.conf.template"),
    ];
    for path in candidates {
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(conf) = ConfFile::parse_bytes(&bytes) else {
            continue;
        };
        let found = conf
            .entries()
            .find(|(k, _)| *k == "Logger.root")
            .and_then(|(_, value)| {
                value
                    .split(',')
                    .next()
                    .map(str::trim)
                    .and_then(|s| s.parse::<u32>().ok())
            });
        if let Some(level) = found {
            return match level {
                0 => Some("off".into()),
                2 => Some("errors".into()),
                4 => Some("normal".into()),
                _ => None,
            };
        }
    }
    None
}

fn log_dirs(root: &Path) -> Vec<PathBuf> {
    let base = owner(root);
    let mut dirs = Vec::new();
    for prefix in ["", ".realms/secondary/", ".realms/reborn/"] {
        let candidate = base.join(format!("{prefix}Core/Logs"));
        if let Ok(checked) = fsx::ensure_within(&base, &candidate) {
            if checked.is_dir() {
                dirs.push(checked);
            }
        }
    }
    dirs
}

fn measure_dir(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            total = total.saturating_add(measure_dir(&path));
        } else if meta.is_file() {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

pub fn measure_logs(root: &Path) -> u64 {
    log_dirs(root)
        .into_iter()
        .fold(0u64, |acc, dir| acc.saturating_add(measure_dir(&dir)))
}

fn clean_dir(dir: &Path, report: &mut CleanLogsReport, remove_empty_dir: bool) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            clean_dir(&path, report, true);
        } else if meta.is_file() {
            let size = meta.len();
            match fs::remove_file(&path) {
                Ok(()) => {
                    report.freed_bytes = report.freed_bytes.saturating_add(size);
                    report.deleted_files = report.deleted_files.saturating_add(1);
                }
                Err(_) => {
                    report.remaining_bytes = report.remaining_bytes.saturating_add(size);
                    report.skipped_files = report.skipped_files.saturating_add(1);
                }
            }
        }
    }
    if remove_empty_dir {
        let _ = fs::remove_dir(dir);
    }
}

pub fn clean_logs(root: &Path) -> Result<CleanLogsReport> {
    let mut report = CleanLogsReport {
        freed_bytes: 0,
        remaining_bytes: 0,
        deleted_files: 0,
        skipped_files: 0,
    };
    for dir in log_dirs(root) {
        clean_dir(&dir, &mut report, false);
    }
    Ok(report)
}

pub fn auto_clean_if_needed(root: &Path) -> Option<CleanLogsReport> {
    let settings = load_settings(root);
    if !settings.auto_cleanup {
        return None;
    }
    let limit = settings.max_size_mb.saturating_mul(1024 * 1024);
    if measure_logs(root) <= limit {
        return None;
    }
    clean_logs(root).ok()
}

pub fn state(root: &Path) -> ServerLoggingView {
    ServerLoggingView {
        preset: detect_preset(root),
        log_bytes: measure_logs(root),
        settings: load_settings(root),
    }
}

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

    #[test]
    fn detects_preset_measures_and_cleans_logs_across_realms() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("Core/configs")).unwrap();
        fs::write(root.join("Core/configs/worldserver.conf"), "Logger.root=4,Console Server\n").unwrap();
        assert_eq!(detect_preset(root).as_deref(), Some("normal"));
        fs::write(root.join("Core/configs/worldserver.conf"), "Logger.root=0,Console Server\n").unwrap();
        assert_eq!(detect_preset(root).as_deref(), Some("off"));
        fs::write(root.join("Core/configs/worldserver.conf"), "Logger.root=2,Console Server\n").unwrap();
        assert_eq!(detect_preset(root).as_deref(), Some("errors"));

        fs::create_dir_all(root.join("Core/Logs/archive")).unwrap();
        fs::create_dir_all(root.join(".realms/secondary/Core/Logs")).unwrap();
        fs::write(root.join("Core/Logs/Server.log"), vec![b'a'; 1000]).unwrap();
        fs::write(root.join("Core/Logs/archive/old.log"), vec![b'b'; 500]).unwrap();
        fs::write(root.join(".realms/secondary/Core/Logs/Server.log"), vec![b'c'; 250]).unwrap();
        assert_eq!(measure_logs(root), 1750);

        let report = clean_logs(root).unwrap();
        assert_eq!(report.freed_bytes, 1750);
        assert_eq!(report.remaining_bytes, 0);
        assert_eq!(report.deleted_files, 3);
        assert_eq!(report.skipped_files, 0);
        assert_eq!(measure_logs(root), 0);
        assert!(!root.join("Core/Logs/archive").exists());
    }

    #[test]
    fn log_settings_validate_and_auto_clean_only_above_threshold() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        assert_eq!(load_settings(root), LogSettings::default());
        assert!(save_settings(root, &LogSettings { auto_cleanup: true, max_size_mb: 5 }).is_err());
        let saved = save_settings(root, &LogSettings { auto_cleanup: true, max_size_mb: 10 }).unwrap();
        assert!(saved.auto_cleanup);
        assert_eq!(saved.max_size_mb, 10);

        fs::create_dir_all(root.join("Core/Logs")).unwrap();
        fs::write(root.join("Core/Logs/Server.log"), vec![b'x'; 1024]).unwrap();
        // 1 KB < 10 MB: does not clean.
        assert!(auto_clean_if_needed(root).is_none());
        assert_eq!(measure_logs(root), 1024);

        // Threshold check helper: writing > 10 MB triggers cleanup.
        let over = vec![b'y'; 10 * 1024 * 1024 + 1];
        fs::write(root.join("Core/Logs/Big.log"), over).unwrap();
        let cleaned = auto_clean_if_needed(root).unwrap();
        assert!(cleaned.freed_bytes > 10 * 1024 * 1024);
        assert_eq!(measure_logs(root), 0);
    }
}
