//! Process identity and observation. A process belongs to an installation only if its executable path and
//! creation time say so; a bare PID or an image name is never trusted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsx;
use crate::layout::Ports;

/// Same shape as the repack's own `.state/<name>.json`, so records are interchangeable with its scripts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub exe: String,
    /// Windows FILETIME of process creation (100 ns ticks since 1601).
    pub created: u64,
}

impl ProcessIdentity {
    pub fn same_as(&self, other: &ProcessIdentity) -> bool {
        self.pid == other.pid && self.created == other.created && self.exe.eq_ignore_ascii_case(&other.exe)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Crashed,
    Updating,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct PortConflict {
    pub port: u16,
    pub pid: u32,
    pub exe: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub name: &'static str,
    pub state: ServiceState,
    pub pid: Option<u32>,
    pub port: u16,
    /// Our process is the one listening on the port.
    pub port_ready: bool,
    /// Something else is listening on our port.
    pub conflict: Option<PortConflict>,
    /// Seconds since the process started.
    pub uptime_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Observed {
    pub mysql: ServiceStatus,
    pub auth: ServiceStatus,
    pub world: ServiceStatus,
}

pub fn read_state_record(root: &Path, name: &str) -> Option<ProcessIdentity> {
    fsx::read_json(&root.join(".state").join(format!("{name}.json"))).ok()
}

#[cfg(windows)]
mod sys {
    use super::ProcessIdentity;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedTcpTable;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn ft(f: FILETIME) -> u64 {
        ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
    }

    pub fn identity(pid: u32) -> Option<ProcessIdentity> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = vec![0u16; 32768];
            let mut size = buf.len() as u32;
            let ok_name = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
            let (mut c, mut e, mut k, mut u) = (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
            let ok_time = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u);
            CloseHandle(h);
            if ok_name == 0 || ok_time == 0 || ft(e) != 0 {
                return None;
            }
            Some(ProcessIdentity { pid, exe: String::from_utf16_lossy(&buf[..size as usize]), created: ft(c) })
        }
    }

    pub fn all_pids() -> Vec<u32> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return out;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut more = Process32FirstW(snap, &mut entry);
            while more != 0 {
                out.push(entry.th32ProcessID);
                more = Process32NextW(snap, &mut entry);
            }
            CloseHandle(snap);
        }
        out
    }

    pub fn now_filetime() -> u64 {
        unsafe {
            let mut f: FILETIME = std::mem::zeroed();
            GetSystemTimeAsFileTime(&mut f);
            ft(f)
        }
    }

    fn table(af: u32) -> Vec<u8> {
        const TCP_TABLE_OWNER_PID_LISTENER: i32 = 3;
        let mut size = 0u32;
        let mut buf: Vec<u8> = Vec::new();
        for _ in 0..4 {
            let code = unsafe {
                GetExtendedTcpTable(
                    if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr().cast() },
                    &mut size,
                    0,
                    af,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            };
            match code {
                0 => return buf,
                122 => buf = vec![0u8; size as usize + 1024],
                _ => return Vec::new(),
            }
        }
        Vec::new()
    }

    /// Every TCP listener with its bind address class, IPv4 and IPv6.
    pub fn listeners_detailed() -> Vec<super::Listener> {
        let mut out = Vec::new();
        let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let v4 = table(2);
        if v4.len() >= 4 {
            for i in 0..u32_at(&v4, 0) as usize {
                let o = 4 + 24 * i;
                if o + 24 > v4.len() {
                    break;
                }
                let addr = u32_at(&v4, o + 4).to_le_bytes(); // network order bytes
                out.push(super::Listener { port: u16::from_be((u32_at(&v4, o + 8) & 0xFFFF) as u16), pid: u32_at(&v4, o + 20), loopback_only: addr[0] == 127 });
            }
        }
        let v6 = table(23);
        if v6.len() >= 4 {
            for i in 0..u32_at(&v6, 0) as usize {
                let o = 4 + 56 * i;
                if o + 56 > v6.len() {
                    break;
                }
                let a = &v6[o..o + 16];
                let loopback = a[..15].iter().all(|b| *b == 0) && a[15] == 1;
                out.push(super::Listener { port: u16::from_be((u32_at(&v6, o + 20) & 0xFFFF) as u16), pid: u32_at(&v6, o + 52), loopback_only: loopback });
            }
        }
        out
    }

    /// (port, owning pid) for every TCP listener, IPv4 and IPv6.
    pub fn listeners() -> Vec<(u16, u32)> {
        let mut out = Vec::new();
        let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let v4 = table(2);
        if v4.len() >= 4 {
            let n = u32_at(&v4, 0) as usize;
            for i in 0..n {
                let o = 4 + 24 * i;
                if o + 24 > v4.len() {
                    break;
                }
                out.push((u16::from_be((u32_at(&v4, o + 8) & 0xFFFF) as u16), u32_at(&v4, o + 20)));
            }
        }
        let v6 = table(23);
        if v6.len() >= 4 {
            let n = u32_at(&v6, 0) as usize;
            for i in 0..n {
                let o = 4 + 56 * i;
                if o + 56 > v6.len() {
                    break;
                }
                out.push((u16::from_be((u32_at(&v6, o + 20) & 0xFFFF) as u16), u32_at(&v6, o + 52)));
            }
        }
        out
    }
}

#[cfg(not(windows))]
mod sys {
    use super::ProcessIdentity;
    pub fn identity(_: u32) -> Option<ProcessIdentity> {
        None
    }
    pub fn all_pids() -> Vec<u32> {
        Vec::new()
    }
    pub fn now_filetime() -> u64 {
        0
    }
    pub fn listeners() -> Vec<(u16, u32)> {
        Vec::new()
    }
    pub fn listeners_detailed() -> Vec<super::Listener> {
        Vec::new()
    }
}

pub use sys::{identity, listeners, listeners_detailed};

#[derive(Debug, Clone)]
pub struct Listener {
    pub port: u16,
    pub pid: u32,
    /// Bound to the loopback address only (not reachable from other computers).
    pub loopback_only: bool,
}

/// True if the recorded identity still describes a live process (same exe and creation time).
pub fn is_alive(record: &ProcessIdentity) -> bool {
    identity(record.pid).map(|cur| cur.same_as(record)).unwrap_or(false)
}

/// Every running process whose image is exactly `exe`.
pub fn find_by_exe(exe: &Path) -> Vec<ProcessIdentity> {
    let want = exe.to_string_lossy().to_lowercase().replace('/', "\\");
    sys::all_pids()
        .into_iter()
        .filter_map(identity)
        .filter(|p| p.exe.to_lowercase() == want)
        .collect()
}

fn uptime_secs(p: &ProcessIdentity) -> Option<u64> {
    let now = sys::now_filetime();
    (now >= p.created).then(|| (now - p.created) / 10_000_000)
}

fn service(root: &Path, name: &'static str, exe: PathBuf, port: u16, listen: &[(u16, u32)]) -> ServiceStatus {
    // Prefer the repack's own record when it still matches a live process; otherwise discover by exact exe path.
    let proc = read_state_record(root, name)
        .filter(is_alive)
        .filter(|r| r.exe.eq_ignore_ascii_case(&exe.to_string_lossy().replace('/', "\\")))
        .or_else(|| find_by_exe(&exe).into_iter().next());

    let owner = listen.iter().find(|(p, _)| *p == port).map(|(_, pid)| *pid);
    let (state, port_ready, conflict) = match (&proc, owner) {
        (Some(p), Some(o)) if p.pid == o => (ServiceState::Running, true, None),
        (Some(_), _) => (ServiceState::Starting, false, None),
        (None, Some(o)) => {
            (ServiceState::Stopped, false, Some(PortConflict { port, pid: o, exe: identity(o).map(|i| i.exe) }))
        }
        (None, None) => (ServiceState::Stopped, false, None),
    };
    ServiceStatus {
        name,
        state,
        pid: proc.as_ref().map(|p| p.pid),
        port,
        port_ready,
        conflict,
        uptime_secs: proc.as_ref().and_then(uptime_secs),
    }
}

/// Snapshot the three services of an installation without touching anything.
pub fn observe(root: &Path, ports: &Ports) -> Observed {
    let listen = listeners();
    let (world, auth, mysql) = crate::layout::executables(root);
    Observed {
        mysql: service(root, "mysql", mysql, ports.mysql, &listen),
        auth: service(root, "auth", auth, ports.auth, &listen),
        world: service(root, "world", world, ports.world, &listen),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn identity_of_self_is_stable_and_alive() {
        let me = identity(std::process::id()).unwrap();
        assert!(me.exe.to_lowercase().ends_with(".exe"));
        assert!(is_alive(&me));
        let mut forged = me.clone();
        forged.created += 1;
        assert!(!is_alive(&forged), "different creation time = different process (PID reuse)");
        let mut wrong_exe = me.clone();
        wrong_exe.exe = "C:\\other.exe".into();
        assert!(!is_alive(&wrong_exe));
        assert!(identity(0xFFFF_FF00).is_none());
    }

    #[test]
    fn detailed_listeners_tell_loopback_from_all_interfaces() {
        let lo = TcpListener::bind("127.0.0.1:0").unwrap();
        let any = TcpListener::bind("0.0.0.0:0").unwrap();
        let (p_lo, p_any) = (lo.local_addr().unwrap().port(), any.local_addr().unwrap().port());
        let all = listeners_detailed();
        assert!(all.iter().any(|l| l.port == p_lo && l.pid == std::process::id() && l.loopback_only));
        assert!(all.iter().any(|l| l.port == p_any && !l.loopback_only));
    }

    #[test]
    fn finds_own_listener_and_exe() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let me = std::process::id();
        assert!(listeners().contains(&(port, me)));
        let exe = identity(me).unwrap().exe;
        assert!(find_by_exe(Path::new(&exe)).iter().any(|p| p.pid == me));
    }

    #[test]
    fn observe_reports_foreign_listener_as_conflict_not_as_our_server() {
        let dir = tempfile::tempdir().unwrap();
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let ports = Ports { mysql: port, auth: 1, world: 2, ra: 3 };
        let o = observe(dir.path(), &ports);
        assert_eq!(o.mysql.state, ServiceState::Stopped);
        assert!(!o.mysql.port_ready);
        assert_eq!(o.mysql.conflict.as_ref().unwrap().pid, std::process::id());
    }

    #[test]
    fn stale_state_record_is_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join(".state");
        std::fs::create_dir_all(&state).unwrap();
        let me = identity(std::process::id()).unwrap();
        // Right PID, wrong creation time => stale record after PID reuse.
        let stale = ProcessIdentity { created: me.created + 12345, ..me };
        std::fs::write(state.join("world.json"), serde_json::to_vec(&stale).unwrap()).unwrap();
        let o = observe(dir.path(), &Ports::default());
        assert_eq!(o.world.state, ServiceState::Stopped);
        assert!(o.world.pid.is_none());
    }
}
