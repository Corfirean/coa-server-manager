//! Starting, checking and stopping the services of one repack: MySQL, authserver, worldserver
//! (`manage.py start_mysql`, `start_auth`, `stop_all` and their helpers), with the same rules.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, ErrorCode, Result};
use crate::launcher::{settings::Settings, state};
use crate::process::ProcessIdentity;

pub type Progress<'a> = &'a dyn Fn(&str);

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    cmd
}

/// Start `exe` with its output appended to `log`, and record it as `name`. `env_remove` are variables of this
/// process the service must not see.
pub fn spawn(root: &Path, name: &str, exe: &Path, args: &[&str], cwd: &Path, log: &Path, env_remove: &[String]) -> Result<ProcessIdentity> {
    let child = crate::launcher::spawn::start(exe, args, cwd, log, env_remove)?;
    state::record(root, name, child.pid)
}

// ---------------------------------------------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------------------------------------------

/// Wait (up to 30 s) until `port` can be bound by a new server. A port with a listening owner is refused at
/// once; a port Windows still holds for a stopped process (a PID-zero row) is waited for. The test bind is
/// exclusive (`SO_EXCLUSIVEADDRUSE`), never a shared one.
pub fn ensure_free(port: u16, progress: Progress) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut announced = false;
    loop {
        let mut owners: Vec<u32> = sys::tcp_rows_v4()
            .into_iter()
            .filter(|r| r.state == sys::LISTEN && r.local_port == port)
            .map(|r| r.pid)
            .collect();
        owners.sort_unstable();
        owners.dedup();
        if owners.iter().any(|p| *p != 0) {
            let list: Vec<String> = owners.iter().map(u32::to_string).collect();
            return Err(Error::Service(
                ErrorCode::PortInUse,
                format!("Port {port} is already used (PID {}). Stop the other server or change Settings/repack.json.", list.join(", ")),
            ));
        }
        match sys::exclusive_bind(port) {
            Ok(()) => return Ok(()),
            Err(code) if code == 10013 || code == 10048 => {
                if Instant::now() >= deadline {
                    return Err(Error::Invalid(format!(
                        "Port {port} remains unavailable after 30 seconds (Windows bind error {code}). No process was stopped. Retry after the previous database shutdown has finished, or check the Windows port reservation."
                    )));
                }
                if !announced {
                    progress(&format!("Waiting for Windows to release port {port}..."));
                    announced = true;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(code) => return Err(Error::Invalid(format!("Port {port} cannot be checked (Windows error {code})."))),
        }
    }
}

/// Is the recorded process of `name` serving `port`? Its listener row is the evidence; when a system does not
/// show the owner of a listener, the server side of a fresh loopback connection is (never for MySQL, whose
/// listener has always been visible and which should not get stray connections).
pub fn ready(root: &Path, name: &str, port: u16) -> bool {
    let Some(record) = state::running(root, name) else { return false };
    let listening = sys::tcp_rows_v4().iter().any(|r| r.state == sys::LISTEN && r.local_port == port && r.pid == record.pid);
    if listening {
        return crate::process::is_alive(&record);
    }
    if name == "mysql" {
        return false;
    }
    accepted_by(port, record.pid) && crate::process::is_alive(&record)
}

/// Connect to 127.0.0.1:`port` and check that exactly one established server-side row, owned by `pid`, answers
/// that connection.
fn accepted_by(port: u16, pid: u32) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let Ok(client) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else { return false };
    let Ok(mine) = client.local_addr().map(|a| a.port()) else { return false };
    let me = std::process::id();
    let rows: Vec<sys::TcpRow> = sys::tcp_rows_v4()
        .into_iter()
        .chain(sys::tcp_rows_v6())
        .filter(|r| r.remote_port == mine && r.remote_loopback && !(r.local_port == mine && r.pid == me))
        .collect();
    rows.len() == 1 && rows[0].state == sys::ESTABLISHED && rows[0].pid == pid
}

pub fn wait_ready(root: &Path, name: &str, port: u16, seconds: u64) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        if ready(root, name, port) {
            return Ok(());
        }
        if state::running(root, name).is_none() {
            return Err(Error::Service(ErrorCode::StartupFailed, format!("{name} stopped during startup. Check its log.")));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err(Error::Service(
        ErrorCode::StartupFailed,
        format!("{name} is still starting: its expected TCP listener on port {port} was not found; check its log."),
    ))
}

// ---------------------------------------------------------------------------------------------------------------
// MySQL client
// ---------------------------------------------------------------------------------------------------------------

/// Run SQL (or a `mysqladmin` command) with the repack's own client and administrator login.
pub fn mysql(root: &Path, sql: Option<&str>, admin: Option<&str>) -> Result<String> {
    let exe = root.join(if admin.is_some() { "mysql/bin/mysqladmin.exe" } else { "mysql/bin/mysql.exe" });
    let defaults = format!("--defaults-file={}", root.join("mysql/admin-client.ini").display());
    let mut cmd = Command::new(exe);
    hidden(&mut cmd).arg(defaults);
    match admin {
        Some(a) => cmd.arg(a),
        None => cmd.args(["--batch", "--skip-column-names"]),
    };
    let payload = sql.map(|s| s.as_bytes().to_vec());
    let timeout = if payload.as_ref().is_some_and(|p| p.len() > 1024 * 1024) { 900 } else { 90 };
    cmd.stdin(if payload.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    if let (Some(data), Some(mut stdin)) = (payload, child.stdin.take()) {
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(&data);
        });
    }
    // Both outputs are read while the client runs: a full pipe would stop it (and look like a timeout).
    let read = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out_reader = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err_reader = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let started = Instant::now();
    let failed = |stderr: &[u8]| {
        let text = String::from_utf8_lossy(stderr);
        let detail = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        let mut message = "The repack database command failed; check mysql/logs/mysql-error.log.".to_string();
        if !detail.is_empty() {
            message.push_str(&format!(" ({detail})"));
        }
        Error::Invalid(message)
    };
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_reader.join();
            return Err(failed(b"timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if !status.success() {
        return Err(failed(&stderr));
    }
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

// ---------------------------------------------------------------------------------------------------------------
// Start and stop
// ---------------------------------------------------------------------------------------------------------------

pub fn start_mysql(root: &Path, cfg: &Settings, progress: Progress) -> Result<()> {
    if cfg.shared_database {
        // An extra world uses the main folder's database; it must already run.
        return mysql(root, None, Some("ping")).map(|_| ()).map_err(|e| {
            Error::Service(ErrorCode::DatabaseNotRunning, format!("The main server's database is not running. {e}"))
        });
    }
    if state::running(root, "mysql").is_some() {
        wait_ready(root, "mysql", cfg.mysql_port, 300)?;
        mysql(root, None, Some("ping"))?;
        progress("MySQL is already running.");
        return Ok(());
    }
    ensure_free(cfg.mysql_port, progress)?;
    if !root.join("mysql/data/mysql").is_dir() {
        return Err(Error::Service(ErrorCode::ServerFilesIncomplete, "The packaged database is missing. Extract the complete repack.".into()));
    }
    progress("Starting MySQL...");
    let ini = format!("--defaults-file={}", root.join("mysql/my.ini").display());
    spawn(root, "mysql", &root.join("mysql/bin/mysqld.exe"), &[&ini, "--no-monitor"], &root.join("mysql"), &root.join("mysql/logs/startup.log"), &cfg.env_remove)?;
    wait_ready(root, "mysql", cfg.mysql_port, 300)?;
    mysql(root, None, Some("ping"))?;
    mysql(
        root,
        Some(&format!(
            "UPDATE acore_auth.realmlist SET name='{}',address='127.0.0.1',localAddress='127.0.0.1',port={} WHERE id={};",
            cfg.realm.name, cfg.world_port, cfg.realm.id
        )),
        None,
    )?;
    Ok(())
}

pub fn start_auth(root: &Path, cfg: &Settings, progress: Progress) -> Result<()> {
    start_mysql(root, cfg, progress)?;
    if state::running(root, "auth").is_some() {
        wait_ready(root, "auth", cfg.auth_port, 60)?;
        progress("Authserver is already running.");
        return Ok(());
    }
    ensure_free(cfg.auth_port, progress)?;
    // An interrupted world startup can leave flag 3, which authserver excludes.
    mysql(root, Some(&format!("UPDATE acore_auth.realmlist SET flag=0 WHERE id={};", cfg.realm.id)), None)?;
    progress("Starting authserver...");
    spawn(root, "auth", &root.join("Core/authserver.exe"), &["-c", "configs/authserver.conf"], &root.join("Core"), &root.join("Core/Logs/auth-console.log"), &cfg.env_remove)?;
    wait_ready(root, "auth", cfg.auth_port, 60)
}

/// Stop the recorded process, only if it is still that process.
pub fn terminate(record: &ProcessIdentity) -> Result<()> {
    if !crate::process::is_alive(record) {
        return Ok(());
    }
    sys::terminate(record)
}

pub fn wait_stopped(record: &ProcessIdentity, seconds: u64) -> bool {
    let until = Instant::now() + Duration::from_secs(seconds);
    while crate::process::is_alive(record) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(250));
    }
    !crate::process::is_alive(record)
}

/// The recorded world, when it is this folder's own `Core/worldserver.exe` (never stop a process only because a
/// record names it).
fn owned_world(root: &Path) -> Result<Option<ProcessIdentity>> {
    let Some(world) = state::running(root, "world") else { return Ok(None) };
    let expected = root.join("Core/worldserver.exe");
    let same = std::fs::canonicalize(&world.exe).ok().zip(std::fs::canonicalize(&expected).ok()).is_some_and(|(a, b)| a == b);
    if !same {
        return Err(Error::Invalid("World process ownership could not be verified; the database was left running.".into()));
    }
    Ok(Some(world))
}

/// Stop this folder's world without its console (unsaved progress since the last save is lost).
fn stop_world_process(world: &ProcessIdentity) -> Result<()> {
    terminate(world).map_err(|_| Error::Invalid("Worldserver could not be stopped safely; its database was left running.".into()))?;
    if !wait_stopped(world, 90) {
        return Err(Error::Invalid("Worldserver is still stopping; its database was left running.".into()));
    }
    Ok(())
}

/// Stop the world at once, then everything else as [`stop_all`] does. Only when the user asked for it.
pub fn force_stop_all(root: &Path, cfg: &Settings, progress: Progress) -> Result<()> {
    if let Some(world) = owned_world(root)? {
        stop_world_process(&world)?;
        progress("Force-stopped this server's verified world process; unsaved progress may be lost.");
    }
    stop_all(root, cfg, progress)
}

/// Stop everything of this repack, world first (gracefully through its console), its database last. An extra
/// world leaves the shared database alone.
pub fn stop_all(root: &Path, cfg: &Settings, progress: Progress) -> Result<()> {
    if let Some(world) = owned_world(root)? {
        let asked = ready(root, "world", cfg.ra_port)
            .then(|| crate::ra::Ra::connect_to(cfg.ra_port, &cfg.ra_username, &cfg.ra_password))
            .and_then(|r| r.ok())
            .map(|ra| ra.send_without_answer("server shutdown 1s 0"));
        match asked {
            Some(Ok(())) => progress("Graceful world shutdown requested."),
            _ => {
                progress("World console unavailable; stopping this repack's recorded world process.");
                stop_world_process(&world)?;
            }
        }
        if !wait_stopped(&world, 90) {
            return Err(Error::Invalid("Worldserver is still stopping; its database has been left running.".into()));
        }
    }
    if let Some(supervisor) = state::running(root, "supervisor") {
        if !wait_stopped(&supervisor, 45) {
            return Err(Error::Invalid("The report relay is still finishing a request. Try Stop All again shortly.".into()));
        }
    }
    if let Some(relay) = state::running(root, "relay") {
        std::fs::write(state::dir(root).join("stop-relay"), b"")?;
        if !wait_stopped(&relay, 40) {
            return Err(Error::Invalid("The report relay did not finish; it was left running for recovery.".into()));
        }
    }
    if let Some(auth) = state::running(root, "auth") {
        terminate(&auth)?;
        if !wait_stopped(&auth, 10) {
            return Err(Error::Invalid("Authserver did not stop.".into()));
        }
    }
    if let Some(database) = state::running(root, "mysql").filter(|_| !cfg.shared_database) {
        let shutdown = mysql(root, None, Some("shutdown"));
        if !wait_stopped(&database, 60) {
            shutdown?;
            return Err(Error::Invalid("MySQL is still stopping; wait before copying the repack.".into()));
        }
    }
    progress("All services from this repack are stopped. It is safe to move or copy this folder.");
    Ok(())
}

/// Which services run (`manage.py status-json`).
pub fn status(root: &Path) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for name in ["mysql", "auth", "world", "relay"] {
        out.insert(name.into(), state::running(root, name).is_some().into());
    }
    let debug: bool = crate::fsx::read_json::<serde_json::Value>(&state::dir(root).join("debug-logs.json"))
        .ok()
        .and_then(|v| v.get("enabled").and_then(|e| e.as_bool()))
        .unwrap_or(false);
    out.insert("debugLogs".into(), debug.into());
    serde_json::Value::Object(out)
}

pub fn log_path(root: &Path, name: &str) -> PathBuf {
    match name {
        "mysql" => root.join("mysql/logs/startup.log"),
        "auth" => root.join("Core/Logs/auth-console.log"),
        "world" => root.join("Core/Logs/world-console.log"),
        "supervisor" => root.join("Core/Logs/supervisor.log"),
        _ => root.join("BugReport/Logs/relay.log"),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------------------------------------------

#[cfg(windows)]
mod sys {
    use crate::error::{Error, Result};
    use crate::process::ProcessIdentity;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedTcpTable;
    use windows_sys::Win32::Networking::WinSock::{setsockopt, SOL_SOCKET, SO_EXCLUSIVEADDRUSE};
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE};

    pub const LISTEN: u32 = 2;
    pub const ESTABLISHED: u32 = 5;

    pub struct TcpRow {
        pub state: u32,
        pub local_port: u16,
        pub remote_port: u16,
        pub remote_loopback: bool,
        pub pid: u32,
    }

    fn table(af: u32) -> Vec<u8> {
        const TCP_TABLE_OWNER_PID_ALL: i32 = 5;
        let mut size = 0u32;
        let mut buf: Vec<u8> = Vec::new();
        for _ in 0..4 {
            let code = unsafe {
                GetExtendedTcpTable(
                    if buf.is_empty() { std::ptr::null_mut() } else { buf.as_mut_ptr().cast() },
                    &mut size,
                    0,
                    af,
                    TCP_TABLE_OWNER_PID_ALL,
                    0,
                )
            };
            match code {
                0 if !buf.is_empty() => return buf,
                0 | 122 => buf = vec![0u8; size as usize + 4096],
                _ => return Vec::new(),
            }
        }
        Vec::new()
    }

    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    pub fn tcp_rows_v4() -> Vec<TcpRow> {
        let b = table(2);
        let mut out = Vec::new();
        if b.len() < 4 {
            return out;
        }
        for i in 0..u32_at(&b, 0) as usize {
            let o = 4 + 24 * i;
            if o + 24 > b.len() {
                break;
            }
            let remote = u32_at(&b, o + 12).to_le_bytes();
            out.push(TcpRow {
                state: u32_at(&b, o),
                local_port: u16::from_be((u32_at(&b, o + 8) & 0xFFFF) as u16),
                remote_port: u16::from_be((u32_at(&b, o + 16) & 0xFFFF) as u16),
                remote_loopback: remote == [127, 0, 0, 1],
                pid: u32_at(&b, o + 20),
            });
        }
        out
    }

    pub fn tcp_rows_v6() -> Vec<TcpRow> {
        let b = table(23);
        let mut out = Vec::new();
        if b.len() < 4 {
            return out;
        }
        // MIB_TCP6ROW_OWNER_PID: local addr 16, scope 4, local port 4, remote addr 16, scope 4, remote port 4, state 4, pid 4
        for i in 0..u32_at(&b, 0) as usize {
            let o = 4 + 56 * i;
            if o + 56 > b.len() {
                break;
            }
            let remote = &b[o + 24..o + 40];
            let mapped_loopback = remote[..10].iter().all(|x| *x == 0) && remote[10] == 0xff && remote[11] == 0xff && remote[12..] == [127, 0, 0, 1];
            out.push(TcpRow {
                state: u32_at(&b, o + 48),
                local_port: u16::from_be((u32_at(&b, o + 20) & 0xFFFF) as u16),
                remote_port: u16::from_be((u32_at(&b, o + 44) & 0xFFFF) as u16),
                remote_loopback: mapped_loopback,
                pid: u32_at(&b, o + 52),
            });
        }
        out
    }

    /// Bind 0.0.0.0:port exclusively and release it. Err carries the Windows socket error.
    pub fn exclusive_bind(port: u16) -> std::result::Result<(), i32> {
        use socket2::{Domain, Socket, Type};
        use std::os::windows::io::AsRawSocket;
        let socket = Socket::new(Domain::IPV4, Type::STREAM, None).map_err(|e| e.raw_os_error().unwrap_or(-1))?;
        let one: i32 = 1;
        let set = unsafe {
            setsockopt(socket.as_raw_socket() as _, SOL_SOCKET, SO_EXCLUSIVEADDRUSE, (&one as *const i32).cast(), 4)
        };
        if set != 0 {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1));
        }
        let addr: std::net::SocketAddr = ([0, 0, 0, 0], port).into();
        socket.bind(&addr.into()).map_err(|e| e.raw_os_error().unwrap_or(-1))
    }

    pub fn terminate(record: &ProcessIdentity) -> Result<()> {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, 0, record.pid);
            if h.is_null() {
                return Err(Error::Invalid("Unable to stop the repack process.".into()));
            }
            let same = crate::process::is_alive(record);
            let ok = same && TerminateProcess(h, 0) != 0;
            CloseHandle(h);
            if !ok {
                return Err(Error::Invalid("The process changed while stopping it.".into()));
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod sys {
    use crate::error::{Error, Result};
    use crate::process::ProcessIdentity;
    pub const LISTEN: u32 = 2;
    pub const ESTABLISHED: u32 = 5;
    pub struct TcpRow {
        pub state: u32,
        pub local_port: u16,
        pub remote_port: u16,
        pub remote_loopback: bool,
        pub pid: u32,
    }
    pub fn tcp_rows_v4() -> Vec<TcpRow> {
        Vec::new()
    }
    pub fn tcp_rows_v6() -> Vec<TcpRow> {
        Vec::new()
    }
    pub fn exclusive_bind(_: u16) -> std::result::Result<(), i32> {
        Ok(())
    }
    pub fn terminate(_: &ProcessIdentity) -> Result<()> {
        Err(Error::Invalid("not supported".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_with_a_listener_is_refused_and_a_free_one_passes() {
        let held = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let err = ensure_free(port, &|_| {}).unwrap_err().to_string();
        assert!(err.contains(&format!("Port {port} is already used (PID {}", std::process::id())), "{err}");
        drop(held);
        ensure_free(port, &|_| {}).unwrap();
    }

    #[test]
    fn a_service_is_ready_only_on_its_own_port() {
        let d = tempfile::tempdir().unwrap();
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(!ready(d.path(), "auth", port), "no record yet");
        state::record(d.path(), "auth", std::process::id()).unwrap();
        assert!(ready(d.path(), "auth", port));
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        drop(other);
        assert!(!ready(d.path(), "auth", port + 1) || port + 1 == 0);
    }

    #[test]
    fn status_reads_the_records() {
        let d = tempfile::tempdir().unwrap();
        state::record(d.path(), "mysql", std::process::id()).unwrap();
        let s = status(d.path());
        assert_eq!(s["mysql"], true);
        assert_eq!(s["world"], false);
        assert_eq!(s["debugLogs"], false);
    }
}
