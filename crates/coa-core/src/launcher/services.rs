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

/// What keeps a new server off a port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortBlock {
    /// Processes listen on it.
    Owned(Vec<u32>),
    /// Windows keeps it in an excluded range (Hyper-V, WSL and Docker reserve such ranges); waiting never helps.
    Reserved(u16, u16),
    /// Windows still refuses it after the wait (a stopped process' socket, or an owner the table does not show).
    Unavailable(i32),
}

/// Wait (up to `wait`) until `port` can be bound by a new server. A port with a listening owner or inside a
/// Windows excluded range is reported at once; a port Windows still holds for a stopped process (a PID-zero row)
/// is waited for. The test bind is exclusive (`SO_EXCLUSIVEADDRUSE`), never a shared one.
pub fn port_block(port: u16, wait: Duration, progress: Progress) -> std::result::Result<(), PortBlock> {
    let deadline = Instant::now() + wait;
    let mut announced = false;
    let mut reserved: Option<Vec<(u16, u16)>> = None;
    loop {
        let mut owners: Vec<u32> = sys::tcp_rows_v4()
            .into_iter()
            .filter(|r| r.state == sys::LISTEN && r.local_port == port)
            .map(|r| r.pid)
            .collect();
        owners.sort_unstable();
        owners.dedup();
        if owners.iter().any(|p| *p != 0) {
            return Err(PortBlock::Owned(owners));
        }
        match sys::exclusive_bind(port) {
            Ok(()) => return Ok(()),
            Err(code) if code == 10013 || code == 10048 => {
                let ranges = reserved.get_or_insert_with(sys::excluded_ranges);
                if let Some((start, end)) = ranges.iter().copied().find(|(s, e)| (*s..=*e).contains(&port)) {
                    return Err(PortBlock::Reserved(start, end));
                }
                if Instant::now() >= deadline {
                    return Err(PortBlock::Unavailable(code));
                }
                if !announced {
                    progress(&format!("Waiting for Windows to release port {port}..."));
                    announced = true;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(code) => return Err(PortBlock::Unavailable(code)),
        }
    }
}

fn port_error(port: u16, block: PortBlock) -> Error {
    match block {
        PortBlock::Owned(owners) => {
            let list: Vec<String> = owners.iter().map(u32::to_string).collect();
            Error::Service(
                ErrorCode::PortInUse,
                format!("Port {port} is already used (PID {}). Stop the other server or change Settings/repack.json.", list.join(", ")),
            )
        }
        PortBlock::Reserved(start, end) => Error::Service(
            ErrorCode::PortInUse,
            format!(
                "Port {port} is reserved by Windows (excluded range {start}-{end}; Hyper-V, WSL and Docker reserve such ranges). Choose another port in Settings/repack.json, or restart the Windows NAT service as administrator (net stop winnat, then net start winnat)."
            ),
        ),
        PortBlock::Unavailable(code) if code == 10013 || code == 10048 => Error::Invalid(format!(
            "Port {port} remains unavailable after 30 seconds (Windows bind error {code}). No process was stopped. Retry after the previous database shutdown has finished, or check the Windows port reservation."
        )),
        PortBlock::Unavailable(code) => Error::Invalid(format!("Port {port} cannot be checked (Windows error {code}).")),
    }
}

/// [`port_block`] with up to 30 s of waiting, as an error a person can act on.
pub fn ensure_free(port: u16, progress: Progress) -> Result<()> {
    port_block(port, Duration::from_secs(30), progress).map_err(|block| port_error(port, block))
}

/// The ports of this server's other services and extra worlds, which the database must not take.
fn ports_in_use_by_server(root: &Path, cfg: &Settings) -> Vec<u16> {
    let mut ports = vec![cfg.auth_port, cfg.world_port, cfg.ra_port];
    for tree in crate::multiworld::Tree::ALL {
        let other = crate::multiworld::tree_root(root, tree);
        if let Ok(other) = Settings::load(&other, 0) {
            ports.extend([other.world_port, other.ra_port, other.auth_port]);
        }
    }
    ports
}

/// Before the database starts: when its port is reserved by Windows or held by a program that is not this
/// server's MySQL, move the database to a free port and record it in `Settings/repack.json`. Only this server's
/// own services connect to MySQL (players never do), and every start renders their configs from that file, so
/// the move needs nothing else. Returns the new port, or `None` when the configured one is usable.
pub fn relocate_database_port(root: &Path, cfg: &Settings, offset: i32, progress: Progress) -> Result<Option<u16>> {
    if cfg.shared_database || offset != 0 || state::running(root, "mysql").is_some() {
        return Ok(None);
    }
    let block = match port_block(cfg.mysql_port, Duration::from_secs(30), progress) {
        Ok(()) => return Ok(None),
        Err(block) => block,
    };
    let ours = root.join("mysql/bin/mysqld.exe");
    if let PortBlock::Owned(owners) = &block {
        // This server's own database without a record: never start a second one on the same data.
        let same = |pid: &u32| {
            crate::process::identity(*pid).is_some_and(|p| same_file(Path::new(&p.exe), &ours))
        };
        if owners.iter().any(same) {
            return Err(port_error(cfg.mysql_port, block));
        }
    }
    let taken = ports_in_use_by_server(root, cfg);
    let reserved = sys::excluded_ranges();
    let candidate = (cfg.mysql_port.saturating_add(1)..=cfg.mysql_port.saturating_add(400))
        .chain(33070..=33470)
        .filter(|p| *p > 1024)
        .find(|p| {
            !taken.contains(p)
                && !reserved.iter().any(|(s, e)| (*s..=*e).contains(p))
                && port_block(*p, Duration::ZERO, &|_| {}).is_ok()
        });
    let Some(port) = candidate else {
        return Err(port_error(cfg.mysql_port, block));
    };
    let reason = match &block {
        PortBlock::Owned(_) => "used by another program",
        PortBlock::Reserved(..) => "reserved by Windows",
        PortBlock::Unavailable(_) => "unavailable",
    };
    let path = root.join("Settings/repack.json");
    let mut raw = cfg.raw.clone();
    raw.insert("mysqlPort".into(), port.into());
    crate::fsx::atomic_write_json(&path, &serde_json::Value::Object(raw))?;
    tracing::warn!(old = cfg.mysql_port, new = port, reason, "database moved to a free port");
    progress(&format!("Port {} is {reason}; the database now uses port {port}.", cfg.mysql_port));
    Ok(Some(port))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
        _ => false,
    }
}

/// The ranges of `netsh interface ipv4 show excludedportrange protocol=tcp`: every line that starts with two
/// port numbers (the headers are translated, the numbers are not).
pub(crate) fn parse_excluded_ranges(text: &str) -> Vec<(u16, u16)> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let start = parts.next()?.parse::<u16>().ok()?;
            let end = parts.next()?.parse::<u16>().ok()?;
            (start <= end).then_some((start, end))
        })
        .collect()
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

/// Ask the world's console for a graceful shutdown. False when the console is not this world's or does not answer.
fn ask_shutdown(root: &Path, cfg: &Settings) -> bool {
    ready(root, "world", cfg.ra_port)
        && crate::ra::Ra::connect_to(cfg.ra_port, &cfg.ra_username, &cfg.ra_password)
            .and_then(|ra| ra.send_without_answer("server shutdown 1s 0"))
            .is_ok()
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
        if ask_shutdown(root, cfg) {
            progress("Graceful world shutdown requested.");
        } else {
            progress("World console unavailable; stopping this repack's recorded world process.");
            stop_world_process(&world)?;
        }
        // A console request can be lost (seen when a client disconnected at once). While the world still runs
        // and its console still answers, it has not begun to stop: ask again. Asking twice is harmless.
        let mut stopped = wait_stopped(&world, 15);
        for _ in 0..2 {
            if stopped {
                break;
            }
            if ask_shutdown(root, cfg) {
                progress("The world has not begun to stop; shutdown requested again.");
            }
            stopped = wait_stopped(&world, 15);
        }
        if !stopped && !wait_stopped(&world, 45) {
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
    /// Windows' excluded TCP port ranges, read with the system's own netsh (never one from PATH or the server).
    pub fn excluded_ranges() -> Vec<(u16, u16)> {
        use std::os::windows::process::CommandExt;
        let mut directory = vec![0u16; 32768];
        let length = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(directory.as_mut_ptr(), directory.len() as u32)
        } as usize;
        if length == 0 || length >= directory.len() {
            return Vec::new();
        }
        let netsh = std::path::PathBuf::from(String::from_utf16_lossy(&directory[..length])).join("netsh.exe");
        let output = std::process::Command::new(netsh)
            .args(["interface", "ipv4", "show", "excludedportrange", "protocol=tcp"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(super::CREATE_NO_WINDOW)
            .output();
        match output {
            Ok(out) if out.status.success() => super::parse_excluded_ranges(&String::from_utf8_lossy(&out.stdout)),
            _ => Vec::new(),
        }
    }

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
    pub fn excluded_ranges() -> Vec<(u16, u16)> {
        Vec::new()
    }
    pub fn terminate(_: &ProcessIdentity) -> Result<()> {
        Err(Error::Invalid("not supported".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_ranges_are_read_from_translated_netsh_output() {
        let english = "\r\nProtocol tcp Port Exclusion Ranges\r\n\r\nStart Port    End Port\r\n----------    --------\r\n      3250        3349\r\n     50000       50059     *\r\n\r\n* - Administered port exclusions.\r\n";
        assert_eq!(parse_excluded_ranges(english), vec![(3250, 3349), (50000, 50059)]);
        let russian = "\r\nПротокол tcp Диапазоны исключения портов\r\n\r\nНачальный порт    Конечный порт\r\n----------    --------\r\n     50000       50059     *\r\n\r\n* — управляемые исключения портов.\r\n";
        assert_eq!(parse_excluded_ranges(russian), vec![(50000, 50059)]);
        assert!(parse_excluded_ranges("").is_empty());
    }

    fn repack(dir: &Path, mysql: u16) {
        std::fs::create_dir_all(dir.join("Settings")).unwrap();
        std::fs::write(
            dir.join("Settings/repack.json"),
            format!(r#"{{"mysqlPort":{mysql},"authPort":3724,"worldPort":8085,"raPort":3443,"raUsername":"U","raPassword":"P"}}"#),
        )
        .unwrap();
    }

    #[test]
    fn a_taken_database_port_moves_to_a_free_one_and_is_recorded() {
        let d = tempfile::tempdir().unwrap();
        let held = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        repack(d.path(), port);
        let cfg = Settings::load(d.path(), 0).unwrap();
        let said = std::cell::RefCell::new(Vec::new());
        let moved = relocate_database_port(d.path(), &cfg, 0, &|line| said.borrow_mut().push(line.to_string()))
            .unwrap()
            .expect("moved");
        assert_ne!(moved, port);
        let again = Settings::load(d.path(), 0).unwrap();
        assert_eq!(again.mysql_port, moved);
        assert_eq!((again.auth_port, again.ra_username.as_str()), (3724, "U"), "other keys are kept");
        assert!(said.borrow().iter().any(|l| l.contains("used by another program")), "{said:?}");
        // The new port is usable: nothing moves on the next start.
        assert_eq!(relocate_database_port(d.path(), &again, 0, &|_| {}).unwrap(), None);
    }

    #[test]
    fn extra_worlds_and_shifted_test_ports_never_move_the_database() {
        let d = tempfile::tempdir().unwrap();
        let held = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();
        repack(d.path(), port);
        let cfg = Settings::load(d.path(), 0).unwrap();
        let shared = Settings { shared_database: true, ..cfg.clone() };
        assert_eq!(relocate_database_port(d.path(), &shared, 0, &|_| {}).unwrap(), None);
        assert_eq!(relocate_database_port(d.path(), &cfg, 5, &|_| {}).unwrap(), None);
        assert_eq!(Settings::load(d.path(), 0).unwrap().mysql_port, port);
    }

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
        // A port nobody listens on now (port + 1 may belong to a test running in parallel in this process).
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let free = other.local_addr().unwrap().port();
        drop(other);
        assert!(!ready(d.path(), "auth", free));
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
