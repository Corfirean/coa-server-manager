//! `.state/`: which processes belong to this repack, and the one-action-at-a-time lock.

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::fsx;
use crate::process::{self, ProcessIdentity};

pub fn dir(root: &Path) -> PathBuf {
    root.join(".state")
}

/// Written like `manage.py atomic_json`: two-space JSON and a newline, now flushed to disk before it replaces
/// the old file.
pub fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fsx::atomic_write(path, text.as_bytes())
}

/// The recorded process of `name` when it is still that process (same executable and start time). A missing,
/// empty or damaged record is no process: `manage.py` failed on those, and every later start with it.
pub fn running(root: &Path, name: &str) -> Option<ProcessIdentity> {
    let record = process::read_state_record(root, name)?;
    process::is_alive(&record).then_some(record)
}

/// Record the process `pid` as `name`. Fails when it has already exited.
pub fn record(root: &Path, name: &str, pid: u32) -> Result<ProcessIdentity> {
    let identity = process::identity(pid)
        .ok_or_else(|| Error::Invalid(format!("{name} exited during startup; check its log.")))?;
    write_json(&dir(root).join(format!("{name}.json")), &identity)?;
    Ok(identity)
}

/// Held while one start/stop action runs. Byte 0 of `.state/control.lock` is locked, exactly the region
/// `msvcrt.locking(fd, LK_NBLCK, 1)` locks in `manage.py`, so a Python launcher and this one exclude each other.
pub struct ControlLock {
    file: File,
    owner: PathBuf,
}

impl ControlLock {
    /// Wait up to `wait` for the lock. `action` is recorded for whoever waits next ("start-all", ...).
    pub fn acquire(root: &Path, action: &str, wait: Duration, progress: &dyn Fn(&str)) -> Result<ControlLock> {
        let state = dir(root);
        std::fs::create_dir_all(&state)?;
        let mut file = OpenOptions::new().read(true).append(true).create(true).open(state.join("control.lock"))?;
        if file.seek(SeekFrom::End(0))? == 0 {
            file.write_all(b"0")?;
            file.flush()?;
        }
        let owner = state.join("control-owner.json");
        let deadline = Instant::now() + wait;
        let mut announced = false;
        loop {
            if sys::try_lock(&file)? {
                break;
            }
            let detail = match fsx::read_json::<serde_json::Value>(&owner) {
                Ok(v) if v.get("pid").is_some() && v.get("action").is_some() => {
                    format!(" PID {}, action {}.", v["pid"], v["action"].as_str().unwrap_or(""))
                }
                _ => " Owner information is unavailable.".into(),
            };
            if Instant::now() >= deadline {
                return Err(Error::Service(crate::error::ErrorCode::OperationInProgress, format!("Another start/stop action is in progress.{detail}")));
            }
            if !announced {
                progress(&format!("Waiting for the current server action.{detail}"));
                announced = true;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let lock = ControlLock { file, owner };
        write_json(
            &lock.owner,
            &serde_json::json!({ "pid": std::process::id(), "action": action, "started": started }),
        )?;
        Ok(lock)
    }
}

impl Drop for ControlLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.owner);
        sys::unlock(&self.file);
    }
}

#[cfg(windows)]
mod sys {
    use std::fs::File;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFile, UnlockFile};

    /// `LockFile` on byte 0, length 1: the call `msvcrt.locking` makes. Non-blocking.
    pub fn try_lock(file: &File) -> std::io::Result<bool> {
        let ok = unsafe { LockFile(file.as_raw_handle() as _, 0, 0, 1, 0) };
        if ok != 0 {
            return Ok(true);
        }
        let err = std::io::Error::last_os_error();
        // ERROR_LOCK_VIOLATION (33): someone else holds it.
        if err.raw_os_error() == Some(33) {
            Ok(false)
        } else {
            Err(err)
        }
    }

    pub fn unlock(file: &File) {
        unsafe {
            UnlockFile(file.as_raw_handle() as _, 0, 0, 1, 0);
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use std::fs::File;
    pub fn try_lock(_: &File) -> std::io::Result<bool> {
        Ok(true)
    }
    pub fn unlock(_: &File) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_damaged_record_is_no_process() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir(d.path())).unwrap();
        std::fs::write(dir(d.path()).join("mysql.json"), [0u8; 116]).unwrap();
        assert!(running(d.path(), "mysql").is_none());
        std::fs::write(dir(d.path()).join("auth.json"), b"").unwrap();
        assert!(running(d.path(), "auth").is_none());
    }

    #[test]
    fn this_process_is_recorded_and_found_running() {
        let d = tempfile::tempdir().unwrap();
        let me = record(d.path(), "probe", std::process::id()).unwrap();
        assert_eq!(running(d.path(), "probe"), Some(me));
        let text = std::fs::read_to_string(dir(d.path()).join("probe.json")).unwrap();
        assert!(text.starts_with("{\n  \"pid\": ") && text.ends_with("}\n"), "{text}");
    }

    #[test]
    fn one_action_at_a_time() {
        let d = tempfile::tempdir().unwrap();
        let first = ControlLock::acquire(d.path(), "start-all", Duration::from_secs(1), &|_| {}).unwrap();
        let owner: serde_json::Value = fsx::read_json(&dir(d.path()).join("control-owner.json")).unwrap();
        assert_eq!(owner["action"], "start-all");
        let said = std::sync::Mutex::new(Vec::new());
        let second = ControlLock::acquire(d.path(), "stop-all", Duration::from_millis(600), &|m| said.lock().unwrap().push(m.to_string()));
        if cfg!(windows) {
            let err = second.err().expect("the second action waits and gives up").to_string();
            assert!(err.contains("Another start/stop action is in progress") && err.contains("start-all"), "{err}");
            assert_eq!(said.lock().unwrap().len(), 1);
        }
        drop(first);
        assert!(!dir(d.path()).join("control-owner.json").exists());
        ControlLock::acquire(d.path(), "stop-all", Duration::from_secs(1), &|_| {}).unwrap();
    }
}
