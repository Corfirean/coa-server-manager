//! Console: read-only views of the server logs and a guarded way to send a command to the world server.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    World,
    Auth,
    Database,
    Manager,
}

pub fn log_path(root: &Path, manager_log: &Path, source: Source) -> PathBuf {
    match source {
        Source::World => root.join("Core/Logs/Server.log"),
        Source::Auth => root.join("Core/Logs/Auth.log"),
        Source::Database => root.join("mysql/logs/mysql-error.log"),
        Source::Manager => manager_log.to_path_buf(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    pub text: String,
    pub level: Level,
}

fn classify(l: &str) -> Level {
    let u = l.to_ascii_uppercase();
    if u.contains(" FATAL ") || u.contains(" ERROR ") || u.contains("[ERROR]") || u.contains("ERROR:") {
        Level::Error
    } else if u.contains(" WARN ") || u.contains("[WARNING]") || u.contains(" WARNING ") {
        Level::Warn
    } else {
        Level::Info
    }
}

/// The last `max_lines` lines of some text (after filtering), classified and redacted like the lines of a log file.
pub fn lines_from_text(text: &str, filter: Option<&str>, max_lines: usize) -> Vec<Line> {
    let needle = filter.map(str::to_lowercase).filter(|n| !n.is_empty());
    let out: Vec<Line> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| needle.as_ref().map(|n| l.to_lowercase().contains(n)).unwrap_or(true))
        .map(|l| Line { level: classify(l), text: crate::diag::redact(l) })
        .collect();
    let skip = out.len().saturating_sub(max_lines);
    out.into_iter().skip(skip).collect()
}

/// The last `max_lines` lines (after filtering) from the last ~512 KB of a log. Works on multi-gigabyte files.
pub fn tail(path: &Path, filter: Option<&str>, max_lines: usize) -> Result<Vec<Line>> {
    const WINDOW: u64 = 512 * 1024;
    let mut f = File::open(path).map_err(|_| Error::Invalid("This log does not exist yet.".into()))?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(WINDOW);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.take(WINDOW).read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // a partial first line
    }
    let needle = filter.map(str::to_lowercase).filter(|n| !n.is_empty());
    let out: Vec<Line> = lines
        .into_iter()
        .filter(|l| needle.as_ref().map(|n| l.to_lowercase().contains(n)).unwrap_or(true))
        .map(|l| Line { level: classify(l), text: crate::diag::redact(l) })
        .collect();
    let skip = out.len().saturating_sub(max_lines);
    Ok(out.into_iter().skip(skip).collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Normal,
    /// Can shut things down, delete or change many records: needs an explicit confirmation.
    Dangerous,
}

/// Commands that must never run without confirmation.
pub fn risk(command: &str) -> Risk {
    let c = command.trim().trim_start_matches('.').to_lowercase();
    const DANGEROUS: [&str; 14] = [
        "server shutdown", "server exit", "server restart", "account delete", "character delete", "character erase", "reset ", "ban ", "unban ", "deleted ", "server idlerestart", "server idleshutdown", "account set password", "reload ",
    ];
    if DANGEROUS.iter().any(|d| c.starts_with(d)) {
        Risk::Dangerous
    } else {
        Risk::Normal
    }
}

/// Validate a command before it goes to the console: one line, printable, bounded.
pub fn check_command(command: &str) -> Result<&str> {
    let c = command.trim();
    if c.is_empty() || c.len() > 500 || c.contains(['\r', '\n', '\0']) {
        return Err(Error::Invalid("Enter a single command.".into()));
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_reads_the_end_of_a_big_file_filters_and_classifies() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("Server.log");
        let mut s = String::new();
        for i in 0..40_000 {
            s.push_str(&format!("2026-09-30 10:00:00 INFO [x] line {i}\n"));
        }
        s.push_str("2026-09-30 10:01:00 ERROR [sql] boom password=abc\n2026-09-30 10:01:01 WARN [x] careful\n");
        std::fs::write(&p, s).unwrap();
        let all = tail(&p, None, 5).unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(all[3].level, Level::Error);
        assert!(!all[3].text.contains("abc"), "secret-looking lines are redacted");
        assert_eq!(all[4].level, Level::Warn);
        let f = tail(&p, Some("CAREFUL"), 100).unwrap();
        assert_eq!(f.len(), 1);
        assert!(tail(&d.path().join("missing.log"), None, 5).is_err());
    }

    #[test]
    fn dangerous_commands_are_recognised_and_input_is_validated() {
        for c in ["server shutdown 10", ".account delete bob", "character delete x", "reset talents", "ban account x 1d y", "reload all"] {
            assert_eq!(risk(c), Risk::Dangerous, "{c}");
        }
        for c in ["server info", ".account onlinelist", "lookup item sword"] {
            assert_eq!(risk(c), Risk::Normal, "{c}");
        }
        assert!(check_command("server info").is_ok());
        for bad in ["", "   ", "a\nb", "a\rb", &"x".repeat(501)] {
            assert!(check_command(bad).is_err());
        }
    }
}
