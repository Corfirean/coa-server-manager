//! Starting a long-running service process that inherits exactly two handles: its log (as stdout and stderr) and
//! NUL (as stdin).
//!
//! `std::process::Command` on Windows lets a child inherit every inheritable handle of the parent. A server
//! started that way keeps open whatever the launcher's caller had open (the Manager's pipes, a test's log file),
//! for as long as the server runs. Python's subprocess passes an explicit handle list; this does the same with
//! `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`.

use std::path::Path;

#[cfg(not(windows))]
use crate::error::Error;
use crate::error::Result;

/// A started process. The handle is closed when this is dropped; the process keeps running.
pub struct Proc {
    pub pid: u32,
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

unsafe impl Send for Proc {}

/// One argument quoted the way the Microsoft C runtime parses it back.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

pub fn command_line(exe: &Path, args: &[&str]) -> String {
    let mut parts = vec![quote(&exe.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote(a)));
    parts.join(" ")
}

#[cfg(windows)]
mod sys {
    use super::Proc;
    use crate::error::{Error, Result};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_ALWAYS, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
        WaitForSingleObject, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    fn open_inheritable(path: &Path, access: u32, disposition: u32) -> Result<HANDLE> {
        let mut sa: SECURITY_ATTRIBUTES = unsafe { std::mem::zeroed() };
        sa.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
        sa.bInheritHandle = 1;
        let name = wide(path.as_os_str());
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &sa,
                disposition,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(h)
    }

    pub fn start(exe: &Path, args: &[&str], cwd: &Path, log: &Path) -> Result<Proc> {
        let out = open_inheritable(log, FILE_APPEND_DATA, OPEN_ALWAYS)?;
        let nul = match open_inheritable(Path::new("NUL"), FILE_GENERIC_READ, OPEN_EXISTING) {
            Ok(h) => h,
            Err(e) => {
                unsafe { CloseHandle(out) };
                return Err(e);
            }
        };
        let result = (|| -> Result<Proc> {
            let mut handles = [out, nul];
            let mut size = 0usize;
            unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size) };
            let mut buf = vec![0u8; size];
            let list: LPPROC_THREAD_ATTRIBUTE_LIST = buf.as_mut_ptr().cast();
            if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut size) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let ok = unsafe {
                UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handles.as_mut_ptr().cast(),
                    std::mem::size_of_val(&handles),
                    std::ptr::null_mut(),
                    std::ptr::null(),
                )
            };
            if ok == 0 {
                let e = std::io::Error::last_os_error();
                unsafe { DeleteProcThreadAttributeList(list) };
                return Err(e.into());
            }
            let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.StartupInfo.hStdInput = nul;
            si.StartupInfo.hStdOutput = out;
            si.StartupInfo.hStdError = out;
            si.lpAttributeList = list;
            let mut cmd = wide(std::ffi::OsStr::new(&super::command_line(exe, args)));
            let dir = wide(cwd.as_os_str());
            let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            let created = unsafe {
                CreateProcessW(
                    std::ptr::null(),
                    cmd.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    1,
                    CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                    std::ptr::null(),
                    dir.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                )
            };
            let error = unsafe { GetLastError() };
            unsafe { DeleteProcThreadAttributeList(list) };
            if created == 0 {
                return Err(Error::Invalid(format!(
                    "{} could not be started (Windows error {error}).",
                    exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                )));
            }
            unsafe { CloseHandle(pi.hThread) };
            Ok(Proc { pid: pi.dwProcessId, handle: pi.hProcess })
        })();
        unsafe {
            CloseHandle(out);
            CloseHandle(nul);
        }
        result
    }

    impl Proc {
        /// The exit code once the process has ended.
        pub fn try_wait(&self) -> Option<u32> {
            unsafe {
                if WaitForSingleObject(self.handle, 0) != WAIT_OBJECT_0 {
                    return None;
                }
                let mut code = 0u32;
                GetExitCodeProcess(self.handle, &mut code);
                Some(code)
            }
        }
    }

    impl Drop for Proc {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }
}

#[cfg(windows)]
pub fn start(exe: &Path, args: &[&str], cwd: &Path, log: &Path) -> Result<Proc> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    sys::start(exe, args, cwd, log)
}

#[cfg(not(windows))]
pub fn start(_: &Path, _: &[&str], _: &Path, _: &Path) -> Result<Proc> {
    Err(Error::Invalid("The repack launcher runs on Windows only.".into()))
}

#[cfg(not(windows))]
impl Proc {
    pub fn try_wait(&self) -> Option<u32> {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_round_trip_through_the_windows_command_line() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote("with space"), "\"with space\"");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }

    #[cfg(windows)]
    #[test]
    fn a_started_process_writes_to_its_log_and_ends() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("out.log");
        let cmd = std::path::PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into()));
        let p = start(&cmd, &["/c", "echo hello & exit 3"], d.path(), &log).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let code = loop {
            if let Some(c) = p.try_wait() {
                break c;
            }
            assert!(std::time::Instant::now() < until);
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        assert_eq!(code, 3);
        assert!(std::fs::read_to_string(&log).unwrap().contains("hello"));
    }
}
