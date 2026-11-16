//! Restarts on a schedule (#202): once a day at the owner's local time, the world supervisor asks the world to
//! restart with an in-game countdown, waits for it to exit with AzerothCore's restart code and starts it again.
//! The database and the auth server keep running.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime, NaiveTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const FILE: &str = "Settings/restart-schedule.json";
/// Present while a scheduled restart is under way, so a world that is briefly down is not taken for a crash.
const UNDER_WAY: &str = ".state/scheduled-restart";
/// The exit code of `server restart` (AzerothCore's `RESTART_EXIT_CODE`).
pub const RESTART_EXIT_CODE: i32 = 2;
/// A restart missed by more than this (the computer slept, the world started late) waits for the next day.
const LATE: Duration = Duration::from_secs(15 * 60);
/// A world that has not come back within this time no longer counts as restarting.
const UNDER_WAY_LIMIT: Duration = Duration::from_secs(45 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartSchedule {
    pub enabled: bool,
    /// Local time, `HH:MM`.
    pub time: String,
    /// The in-game countdown before the restart, in minutes (1-60).
    pub warning_minutes: u32,
}

impl Default for RestartSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            time: "05:00".into(),
            warning_minutes: 5,
        }
    }
}

impl RestartSchedule {
    fn at(&self) -> Option<NaiveTime> {
        NaiveTime::parse_from_str(self.time.trim(), "%H:%M").ok()
    }

    fn validate(&self) -> Result<()> {
        if self.at().is_none() {
            return Err(Error::Invalid("The restart time must be HH:MM, for example 05:00.".into()));
        }
        if !(1..=60).contains(&self.warning_minutes) {
            return Err(Error::Invalid("The restart warning must be 1 to 60 minutes.".into()));
        }
        Ok(())
    }

    /// The scheduled moment of `now`'s day, when a restart is due at `now` for a world started at `started`:
    /// the moment has passed by less than [`LATE`] and the world was already running before it.
    pub fn due(&self, now: NaiveDateTime, started: NaiveDateTime) -> Option<NaiveDateTime> {
        if !self.enabled {
            return None;
        }
        let moment = now.date().and_time(self.at()?);
        let late = now.signed_duration_since(moment).to_std().ok()?;
        (late <= LATE && started < moment).then_some(moment)
    }
}

/// The folder whose schedule a world follows: an extra world (`<server>/.realms/<name>`) follows its server's.
fn owner(root: &Path) -> PathBuf {
    if crate::launcher::settings::is_extra_world(root) {
        if let Some(main) = root.parent().and_then(Path::parent) {
            return main.to_path_buf();
        }
    }
    root.to_path_buf()
}

pub fn load(root: &Path) -> RestartSchedule {
    crate::fsx::read_json(&owner(root).join(FILE)).unwrap_or_default()
}

pub fn save(root: &Path, schedule: &RestartSchedule) -> Result<RestartSchedule> {
    let mut schedule = schedule.clone();
    schedule.time = schedule.time.trim().to_string();
    if schedule.enabled {
        schedule.validate()?;
    } else if schedule.at().is_none() {
        schedule.time = RestartSchedule::default().time;
    }
    crate::fsx::atomic_write_json(&owner(root).join(FILE), &schedule)?;
    Ok(schedule)
}

pub fn now() -> NaiveDateTime {
    Local::now().naive_local().with_nanosecond(0).unwrap_or_else(|| Local::now().naive_local())
}

pub fn mark_under_way(root: &Path) -> Result<()> {
    let path = root.join(UNDER_WAY);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, b"")?;
    Ok(())
}

pub fn clear_under_way(root: &Path) {
    let _ = std::fs::remove_file(root.join(UNDER_WAY));
}

/// A scheduled restart of this world is under way (its world may be down for a few minutes).
pub fn under_way(root: &Path) -> bool {
    std::fs::metadata(root.join(UNDER_WAY))
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < UNDER_WAY_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn a_restart_is_due_once_after_its_time_for_a_world_that_ran_before_it() {
        let s = RestartSchedule { enabled: true, time: "05:00".into(), warning_minutes: 5 };
        let started = at("2026-11-14 20:00");
        assert_eq!(s.due(at("2026-11-15 04:59"), started), None);
        assert_eq!(s.due(at("2026-11-15 05:00"), started), Some(at("2026-11-15 05:00")));
        assert_eq!(s.due(at("2026-11-15 05:14"), started), Some(at("2026-11-15 05:00")));
        assert_eq!(s.due(at("2026-11-15 05:16"), started), None, "missed by too much");
        // The world that came back at 05:06 is not restarted again that day.
        assert_eq!(s.due(at("2026-11-15 05:10"), at("2026-11-15 05:06")), None);
        assert_eq!(s.due(at("2026-11-16 05:01"), at("2026-11-15 05:06")), Some(at("2026-11-16 05:00")));
        let off = RestartSchedule { enabled: false, ..s };
        assert_eq!(off.due(at("2026-11-15 05:00"), started), None);
    }

    #[test]
    fn schedules_are_validated_and_extra_worlds_follow_their_server() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(d.path()), RestartSchedule::default());
        let bad = RestartSchedule { enabled: true, time: "25:00".into(), warning_minutes: 5 };
        assert!(save(d.path(), &bad).is_err());
        let quiet = RestartSchedule { enabled: true, time: "05:00".into(), warning_minutes: 0 };
        assert!(save(d.path(), &quiet).is_err());
        let good = RestartSchedule { enabled: true, time: " 04:30 ".into(), warning_minutes: 10 };
        assert_eq!(save(d.path(), &good).unwrap().time, "04:30");
        let extra = d.path().join(".realms/secondary");
        std::fs::create_dir_all(&extra).unwrap();
        assert!(load(&extra).enabled);
        assert!(!under_way(d.path()));
        mark_under_way(d.path()).unwrap();
        assert!(under_way(d.path()));
        clear_under_way(d.path());
        assert!(!under_way(d.path()));
    }
}
