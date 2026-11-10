//! `prepare`: the configuration files a repack runs with, rendered from `Settings/*.template` and
//! `Settings/repack.json` (`manage.py prepare()`), byte for byte the same.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::launcher::{settings::Settings, state};

/// Services whose running process blocks moving the folder or changing ports.
const SERVICES: [&str; 5] = ["mysql", "auth", "world", "supervisor", "relay"];

/// The root as the servers see it: forward slashes, and the short (8.3) form when the real one has non-Latin
/// letters, which MySQL and the servers do not read reliably from their config files.
pub fn native_root(root: &Path) -> Result<String> {
    let mut value = root.to_string_lossy().into_owned();
    if !value.is_ascii() {
        if let Some(short) = sys::short_path(root) {
            value = short;
        }
    }
    if !value.is_ascii() {
        return Err(Error::Invalid(
            "Extract the repack to a path with Latin letters, such as C:\\Games\\CoA-Repack.".into(),
        ));
    }
    Ok(value.replace('\\', "/"))
}

/// Text as Python's text mode reads it: strict UTF-8, Windows and old Mac line ends turned into "\n".
fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8(bytes).map_err(|_| Error::Invalid(format!("{} is not UTF-8 text.", path.display())))?;
    Ok(text.replace("\r\n", "\n").replace('\r', "\n"))
}

/// Text written as Python's text mode writes it on Windows: "\n" becomes "\r\n".
fn windows_text(text: &str) -> Vec<u8> {
    if cfg!(windows) {
        text.replace('\n', "\r\n").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

fn placeholders(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphabetic() || bytes[j] == b'_') {
                j += 1;
            }
            if j < bytes.len() && j > i + 1 && bytes[j] == b'@' {
                out.push(line[i..=j].to_string());
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn credentials(root: &Path) -> Result<(String, String)> {
    let v: serde_json::Value = crate::fsx::read_json(&root.join("Settings/database.json"))?;
    let good = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .filter(|s| s.len() == 48 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            .map(str::to_string)
    };
    match (good("rootPassword"), good("appPassword")) {
        (Some(r), Some(a)) => Ok((r, a)),
        _ => Err(Error::Invalid("Invalid packaged database configuration.".into())),
    }
}

/// Render every file the repack's services read. Refuses while a service runs and the folder or the ports
/// changed since the last render.
pub fn prepare(root: &Path, config: &Settings) -> Result<Vec<PathBuf>> {
    let base = native_root(root)?;
    let signature = serde_json::json!({
        "root": root.to_string_lossy(),
        "ports": serde_json::Value::Object(config.port_map()),
    });
    let previous = state::dir(root).join("configuration.json");
    if previous.exists() {
        let recorded: Option<serde_json::Value> = crate::fsx::read_json(&previous).ok();
        if recorded.as_ref() != Some(&signature) && SERVICES.iter().any(|s| state::running(root, s).is_some()) {
            return Err(Error::Invalid("Stop this repack before moving its folder or changing its ports.".into()));
        }
    }
    let (root_password, app_password) = credentials(root)?;
    for folder in ["Core/configs/modules", "Core/Logs", "BugReport/reports", "BugReport/Logs", "mysql/logs", ".state"] {
        std::fs::create_dir_all(root.join(folder))?;
    }
    let mysql_port = config.mysql_port;
    let mut values: Vec<(&str, String)> = vec![
        ("DATA", format!("{base}/Data")),
        ("LOGS", format!("{base}/Core/Logs")),
        ("ASCENSION_DBC", format!("{base}/Data/dbc/Ascension")),
        ("MYSQL_EXE", format!("{base}/mysql/bin/mysql.exe")),
        ("AUTH_PORT", config.auth_port.to_string()),
        ("WORLD_PORT", config.world_port.to_string()),
        ("RA_PORT", config.ra_port.to_string()),
    ];
    for (key, db) in [("LoginDatabaseInfo", "acore_auth"), ("WorldDatabaseInfo", "acore_world"), ("CharacterDatabaseInfo", "acore_characters")] {
        values.push((key, format!("127.0.0.1;{mysql_port};acore;{app_password};{db}")));
    }
    let mut written = Vec::new();
    for (name, target) in [
        ("authserver.conf", "Core/configs/authserver.conf"),
        ("worldserver.conf", "Core/configs/worldserver.conf"),
        ("mod_ascension_compat.conf", "Core/configs/modules/mod_ascension_compat.conf"),
    ] {
        let mut text = read_text(&root.join("Settings").join(format!("{name}.template")))?;
        for (key, value) in &values {
            text = text.replace(&format!("@{key}@"), value);
        }
        let mut unresolved: Vec<String> = text
            .split('\n')
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
            .flat_map(placeholders)
            .collect();
        unresolved.sort();
        unresolved.dedup();
        if !unresolved.is_empty() {
            return Err(Error::Invalid(format!(
                "An unresolved placeholder remains in Settings/{name}.template: {}",
                unresolved.join(", ")
            )));
        }
        // these three keep "\n" (Python wrote them with newline="\n")
        std::fs::write(root.join(target), text.as_bytes())?;
        written.push(root.join(target));
    }
    let files: [(&str, String); 3] = [
        (
            "Core/configs/modules/coa_bugreport.conf",
            format!(
                "# Managed by the repack launcher. Enabled whenever worldserver starts.\nCoABugReport.Enable = 1\nCoABugReport.SpoolDirectory = \"{base}/BugReport/reports\"\nCoABugReport.CooldownSeconds = 120\n"
            ),
        ),
        (
            "mysql/my.ini",
            format!(
                "[mysqld]\nbasedir=\"{base}/mysql\"\ndatadir=\"{base}/mysql/data\"\nport={mysql_port}\nbind-address=127.0.0.1\nmysqlx=0\ncharacter-set-server=utf8mb4\ncollation-server=utf8mb4_unicode_ci\nmax_allowed_packet=128M\ninnodb_buffer_pool_size=256M\nskip-log-bin\nlog-error=\"{base}/mysql/logs/mysql-error.log\"\npid-file=\"{base}/mysql/mysql.pid\"\n"
            ),
        ),
        (
            "mysql/admin-client.ini",
            format!(
                "[client]\nuser=root\npassword={root_password}\nhost=127.0.0.1\nport={mysql_port}\nprotocol=tcp\ndefault-character-set=utf8mb4\n"
            ),
        ),
    ];
    for (rel, text) in files {
        std::fs::write(root.join(rel), windows_text(&text))?;
        written.push(root.join(rel));
    }
    state::write_json(&previous, &signature)?;
    Ok(written)
}

#[cfg(windows)]
mod sys {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    pub fn short_path(path: &Path) -> Option<String> {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut buf = vec![0u16; 32768];
        let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        (n > 0 && (n as usize) < buf.len()).then(|| String::from_utf16_lossy(&buf[..n as usize]))
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn short_path(_: &std::path::Path) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn fixture(root: &Path) {
        let w = |rel: &str, text: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        w("Settings/repack.json", br#"{"authPort": 3724, "mysqlPort": 3307, "raPassword": "e0c39beaa5c14128", "raPort": 3443, "raUsername": "COAMANAGER", "worldPort": 8085}"#);
        w("Settings/database.json", format!(r#"{{"rootPassword": "{}", "appPassword": "{}"}}"#, "a".repeat(48), "0123456789abcdef".repeat(3)).as_bytes());
        w("Settings/authserver.conf.template", b"[authserver]\r\nLoginDatabaseInfo = \"@LoginDatabaseInfo@\"\r\nRealmServerPort = @AUTH_PORT@\r\nLogsDir = \"@LOGS@\"\r\n# @NOT_A_VALUE@ in a comment is fine\r\n");
        w("Settings/worldserver.conf.template", "[worldserver]\nDataDir = \"@DATA@\"\nWorldServerPort = @WORLD_PORT@\nRa.Port = @RA_PORT@\nWorldDatabaseInfo = \"@WorldDatabaseInfo@\"\nCharacterDatabaseInfo = \"@CharacterDatabaseInfo@\"\nMySQLExecutable = \"@MYSQL_EXE@\"\nMotd = \"Ünïcode stays\"\n".as_bytes());
        w("Settings/mod_ascension_compat.conf.template", b"Ascension.DbcDir = \"@ASCENSION_DBC@\"\rOld.Mac = 1\r");
    }

    #[test]
    fn placeholders_left_outside_comments_are_refused() {
        let d = tempfile::tempdir().unwrap();
        fixture(d.path());
        std::fs::write(d.path().join("Settings/worldserver.conf.template"), "A = @UNKNOWN@\n").unwrap();
        let cfg = Settings::load(d.path(), 0).unwrap();
        let err = prepare(d.path(), &cfg).unwrap_err().to_string();
        assert!(err.contains("worldserver.conf.template: @UNKNOWN@"), "{err}");
    }

    #[test]
    fn a_bad_password_file_is_refused() {
        let d = tempfile::tempdir().unwrap();
        fixture(d.path());
        std::fs::write(d.path().join("Settings/database.json"), r#"{"rootPassword":"short","appPassword":"x"}"#).unwrap();
        let cfg = Settings::load(d.path(), 0).unwrap();
        assert!(prepare(d.path(), &cfg).unwrap_err().to_string().contains("Invalid packaged database configuration"));
    }

    /// Byte-for-byte the same files as the Python launcher. COA_MANAGE_PY = a manage.py to compare with (taken from
    /// coa-server-build); skipped without it.
    #[test]
    fn renders_exactly_what_manage_py_renders() {
        let Ok(manage) = std::env::var("COA_MANAGE_PY") else { return };
        let python = std::env::var("COA_PYTHON").unwrap_or_else(|_| "python".into());
        let rust = tempfile::tempdir().unwrap();
        let py = tempfile::tempdir().unwrap();
        for d in [rust.path(), py.path()] {
            fixture(d);
        }
        std::fs::create_dir_all(py.path().join("Scripts")).unwrap();
        std::fs::copy(&manage, py.path().join("Scripts/manage.py")).unwrap();
        // manage.py imports squid_playerbots lazily; prepare does not need it.
        let out = std::process::Command::new(&python)
            .args(["-I", "-B"])
            .arg(py.path().join("Scripts/manage.py"))
            .arg("prepare")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let cfg = Settings::load(rust.path(), 0).unwrap();
        let written = prepare(rust.path(), &cfg).unwrap();
        let root_text = |p: &Path, base: &Path| {
            let n = native_root(base).unwrap();
            String::from_utf8(std::fs::read(p).unwrap()).unwrap().replace(&n, "<ROOT>")
        };
        for file in &written {
            let rel = file.strip_prefix(rust.path()).unwrap();
            assert_eq!(
                root_text(file, rust.path()),
                root_text(&py.path().join(rel), py.path()),
                "{} differs",
                rel.display()
            );
        }
        let sig = |base: &Path| -> serde_json::Value {
            let mut v: serde_json::Value = crate::fsx::read_json(&base.join(".state/configuration.json")).unwrap();
            v["root"] = serde_json::Value::Null;
            v
        };
        assert_eq!(sig(rust.path()), sig(py.path()));
    }
}
