//! Start, stop and observe the three containers of a Docker installation.
//!
//! Start order matters: the database must be healthy before the game servers, and the world server must be listening
//! before the auth server, because the auth server refuses to run while no realm is online.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::cli::{Call, Docker, SystemDocker};
use super::{runtime_image, Config, Names, RUNTIME_DOCKERFILE};
use crate::driver::{DriverOutcome, Verb};
use crate::error::{ErrorCode, Result};
use crate::layout::{read_ports, Ports};
use crate::process::{Observed, ServiceState, ServiceStatus};

/// Where the server folder and the game data appear inside the containers.
const CORE: &str = "/srv/core";
const DATA: &str = "/srv/data";
/// Fixed ports inside the containers; the host side comes from `Settings/repack.json`.
const AUTH_PORT: u16 = 3724;
const WORLD_PORT: u16 = 8085;
const RA_PORT: u16 = 3443;

const QUICK: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_secs(1);
const DB_WAIT: u32 = 120;
const WORLD_WAIT: u32 = 300;
const AUTH_WAIT: u32 = 60;

struct Failure {
    code: ErrorCode,
    output: String,
}

type Step<T> = std::result::Result<T, Failure>;

#[derive(Default)]
struct Log(Vec<String>);

impl Log {
    fn say(&mut self, line: impl Into<String>) {
        self.0.push(line.into());
    }
}

fn fail<T>(code: ErrorCode, output: impl Into<String>) -> Step<T> {
    Err(Failure { code, output: output.into() })
}

pub fn run(root: &Path, verb: Verb) -> Result<DriverOutcome> {
    run_with(&SystemDocker, root, verb)
}

pub fn run_with(d: &dyn Docker, root: &Path, verb: Verb) -> Result<DriverOutcome> {
    let cfg = Config::load(root)?;
    let mut log = Log::default();
    let result = match verb {
        Verb::StartWorld => return Err(crate::Error::Invalid("Starting a secondary world is not supported for Docker servers.".into())),
        Verb::StartAll => start(d, root, &cfg, true, &mut log),
        Verb::StartMysql => start(d, root, &cfg, false, &mut log),
        Verb::StopAll => stop(d, &cfg, &mut log),
    };
    Ok(match result {
        Ok(()) => DriverOutcome { ok: true, exit_code: Some(0), code: None, human: None, output: log.0.join("\n") },
        Err(f) => {
            log.say(f.output);
            DriverOutcome { ok: false, exit_code: None, code: Some(f.code), human: Some(f.code.human()), output: log.0.join("\n") }
        }
    })
}

// ---------------------------------------------------------------------------------------------------- docker calls

/// Run docker; only "docker cannot be started" is an error here, a non-zero exit is returned for the caller to judge.
fn docker(d: &dyn Docker, args: &[&str], timeout: Duration) -> Step<super::cli::Output> {
    d.run(&Call::new(args, timeout)).or_else(|e| fail(ErrorCode::DockerUnavailable, e.to_string()))
}

/// Is docker usable by this user? Returns its version, or the reason in docker's own words.
pub fn check_docker(d: &dyn Docker) -> Result<String> {
    let mut log = Log::default();
    match preflight(d, &mut log) {
        Ok(()) => Ok(log.0.join(" ")),
        Err(f) => Err(crate::error::Error::Invalid(format!("{} {}", f.code.human().message, f.output.trim()).trim().to_string())),
    }
}

/// Remove everything docker holds for an installation: containers, network and the database volume. Used when an
/// installation fails before it is finished, and later to uninstall. Missing pieces are not an error.
pub(crate) fn destroy(d: &dyn Docker, cfg: &Config) {
    let n = cfg.names();
    for name in [&n.auth, &n.world, &n.db] {
        let _ = d.run(&Call::new(&["rm", "--force", name], QUICK));
    }
    let _ = d.run(&Call::new(&["network", "rm", &n.network], QUICK));
    let _ = d.run(&Call::new(&["volume", "rm", &n.volume], QUICK));
}

fn preflight(d: &dyn Docker, log: &mut Log) -> Step<()> {
    let o = docker(d, &["version", "--format", "{{.Server.Version}}"], QUICK)?;
    if !o.ok() {
        // Typical causes: the service is stopped, or the user is not allowed to talk to it.
        return fail(ErrorCode::DockerUnavailable, o.text());
    }
    log.say(format!("docker {}", o.stdout.trim()));
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Container {
    pub status: String,
    pub exit_code: i64,
    pub oom_killed: bool,
    pub pid: Option<u32>,
    pub started_at: Option<String>,
    /// "healthy", "unhealthy" or "starting" when the container has a health check.
    pub health: Option<String>,
}

impl Container {
    fn running(&self) -> bool {
        self.status == "running"
    }
}

/// State of the named containers that exist (the others are simply absent from the result).
fn inspect(d: &dyn Docker, names: &[&str]) -> Step<HashMap<String, Container>> {
    let mut args = vec!["inspect"];
    args.extend_from_slice(names);
    let o = docker(d, &args, QUICK)?;
    // docker prints what it found and complains about the rest with a non-zero exit code.
    let found = parse_inspect(&o.stdout);
    if !o.ok() && found.is_empty() && !o.stderr.contains("No such object") && !o.stderr.contains("no such object") {
        return fail(ErrorCode::DockerUnavailable, o.text());
    }
    Ok(found)
}

pub(crate) fn parse_inspect(json: &str) -> HashMap<String, Container> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else { return HashMap::new() };
    items
        .iter()
        .filter_map(|v| {
            let name = v["Name"].as_str()?.trim_start_matches('/').to_string();
            let s = &v["State"];
            Some((
                name,
                Container {
                    status: s["Status"].as_str()?.to_string(),
                    exit_code: s["ExitCode"].as_i64().unwrap_or(0),
                    oom_killed: s["OOMKilled"].as_bool().unwrap_or(false),
                    pid: s["Pid"].as_u64().filter(|p| *p > 0).map(|p| p as u32),
                    started_at: s["StartedAt"].as_str().map(str::to_string),
                    health: s["Health"]["Status"].as_str().map(str::to_string),
                },
            ))
        })
        .collect()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Last lines a container wrote, as the technical details of a failure, and the cause they point to.
fn why_stopped(d: &dyn Docker, name: &str) -> (ErrorCode, String) {
    let text = d.run(&Call::new(&["logs", "--tail", "60", name], QUICK)).map(|o| strip_ansi(&o.text())).unwrap_or_default();
    (crate::health::diagnose(&text).unwrap_or(ErrorCode::StartupFailed), format!("--- last lines of {name}\n{text}"))
}

// ------------------------------------------------------------------------------------------------------------ start

struct Secrets {
    root: String,
    app: String,
}

fn start(d: &dyn Docker, root: &Path, cfg: &Config, with_game: bool, log: &mut Log) -> Step<()> {
    preflight(d, log)?;
    let n = cfg.names();
    let secrets = crate::db::credentials(root).map(|(root, app)| Secrets { root, app }).or_else(|e| fail(ErrorCode::ServerFilesIncomplete, e.to_string()))?;
    let ports = read_ports(root);
    let state = inspect(d, &[&n.db, &n.world, &n.auth])?;

    if !docker(d, &["network", "inspect", &n.network], QUICK)?.ok() {
        let o = docker(d, &["network", "create", &n.network], QUICK)?;
        if !o.ok() {
            return fail(ErrorCode::StartupFailed, o.text());
        }
        log.say(format!("created network {}", n.network));
    }

    if !state.get(&n.db).is_some_and(Container::running) {
        remove(d, &n.db);
        let mut call = Call::new(&[], Duration::from_secs(120));
        call.args = db_args(cfg, &n);
        call.env = vec![("MYSQL_ROOT_PASSWORD".into(), secrets.root.clone())];
        let o = d.run(&call).or_else(|e| fail(ErrorCode::DockerUnavailable, e.to_string()))?;
        if !o.ok() {
            return fail(ErrorCode::StartupFailed, o.text());
        }
        log.say(format!("started {}", n.db));
    }
    wait_database(d, &n, log)?;
    if !with_game {
        return Ok(());
    }

    ensure_image(d, log)?;
    super::ensure_main_configs(root).map_err(|e| Failure { code: ErrorCode::ServerFilesIncomplete, output: e.to_string() })?;
    std::fs::create_dir_all(root.join("Core/Logs")).map_err(|e| Failure { code: ErrorCode::ServerFilesIncomplete, output: e.to_string() })?;
    let owner = owner_of(root);
    let host = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let host = crate::fsx::canonicalize_lenient(&host).unwrap_or(host);

    if !state.get(&n.world).is_some_and(Container::running) {
        remove(d, &n.world);
        let mut call = Call::new(&[], Duration::from_secs(60));
        call.args = game_args(cfg, &n, GameKind::World, &host, &cfg.data_path(&host), ports.world, ports.ra, owner.as_deref());
        call.env = database_env(&secrets, true);
        run_container(d, &call, &n.world, log)?;
    }
    wait_listening(d, cfg, &n.world, ports.world, WORLD_WAIT, log)?;

    if !state.get(&n.auth).is_some_and(Container::running) {
        remove(d, &n.auth);
        let mut call = Call::new(&[], Duration::from_secs(60));
        call.args = game_args(cfg, &n, GameKind::Auth, &host, &cfg.data_path(&host), ports.auth, 0, owner.as_deref());
        call.env = database_env(&secrets, false);
        run_container(d, &call, &n.auth, log)?;
    }
    wait_listening(d, cfg, &n.auth, ports.auth, AUTH_WAIT, log)?;
    Ok(())
}

fn remove(d: &dyn Docker, name: &str) {
    // A stopped container of the same name is replaced, so its settings always match the files on disk.
    let _ = d.run(&Call::new(&["rm", "-f", name], QUICK));
}

fn run_container(d: &dyn Docker, call: &Call, name: &str, log: &mut Log) -> Step<()> {
    let o = d.run(call).or_else(|e| fail(ErrorCode::DockerUnavailable, e.to_string()))?;
    if !o.ok() {
        let text = o.text();
        let code = if text.contains("port is already allocated") || text.contains("address already in use") { ErrorCode::PortInUse } else { ErrorCode::StartupFailed };
        return fail(code, text);
    }
    log.say(format!("started {name}"));
    Ok(())
}

fn ensure_image(d: &dyn Docker, log: &mut Log) -> Step<()> {
    let image = runtime_image();
    if docker(d, &["image", "inspect", &image], QUICK)?.ok() {
        return Ok(());
    }
    log.say(format!("building {image} (first start only)"));
    let mut call = Call::new(&["build", "--tag", &image, "-"], Duration::from_secs(900));
    call.stdin = Some(RUNTIME_DOCKERFILE.as_bytes());
    let o = d.run(&call).or_else(|e| fail(ErrorCode::DockerUnavailable, e.to_string()))?;
    if !o.ok() {
        return fail(ErrorCode::StartupFailed, o.text());
    }
    Ok(())
}

fn wait_database(d: &dyn Docker, n: &Names, log: &mut Log) -> Step<()> {
    for _ in 0..DB_WAIT {
        let state = inspect(d, &[&n.db])?;
        match state.get(&n.db) {
            Some(c) if c.running() && c.health.as_deref() == Some("healthy") => {
                log.say("database is ready");
                return Ok(());
            }
            Some(c) if !c.running() => {
                let (code, text) = why_stopped(d, &n.db);
                return fail(if code == ErrorCode::StartupFailed { ErrorCode::DatabaseNotRunning } else { code }, text);
            }
            None => return fail(ErrorCode::DatabaseNotRunning, "the database container does not exist"),
            _ => d.pause(POLL),
        }
    }
    fail(ErrorCode::DatabaseNotRunning, "the database did not become ready in time")
}

/// Wait until the container accepts connections on its published port; give up when it stops or on timeout.
fn wait_listening(d: &dyn Docker, cfg: &Config, name: &str, port: u16, seconds: u32, log: &mut Log) -> Step<()> {
    let addr = SocketAddr::new(connect_ip(cfg), port);
    for _ in 0..seconds {
        let state = inspect(d, &[name])?;
        match state.get(name) {
            Some(c) if c.running() => {
                if d.port_open(addr) {
                    log.say(format!("{name} is listening on port {port}"));
                    return Ok(());
                }
                d.pause(POLL);
            }
            _ => {
                let (code, text) = why_stopped(d, name);
                return fail(code, text);
            }
        }
    }
    let (code, text) = why_stopped(d, name);
    fail(code, format!("{name} was not listening on port {port} after {seconds} seconds\n{text}"))
}

/// Address to test a published port on from this computer.
fn connect_ip(cfg: &Config) -> IpAddr {
    match cfg.bind_address.parse::<IpAddr>() {
        Ok(ip) if !ip.is_unspecified() => ip,
        _ => IpAddr::V4(Ipv4Addr::LOCALHOST),
    }
}

// ------------------------------------------------------------------------------------------------- argument lists

fn db_args(cfg: &Config, n: &Names) -> Vec<String> {
    let mut a: Vec<String> = ["run", "--detach", "--name"].iter().map(|s| s.to_string()).collect();
    a.push(n.db.clone());
    a.extend(["--network".into(), n.network.clone(), "--network-alias".into(), "db".into()]);
    a.extend(["--label".into(), format!("coa.project={}", cfg.project)]);
    a.extend(["--volume".into(), format!("{}:/var/lib/mysql", n.volume)]);
    // The password comes from the environment of the docker client, not from this command line.
    a.extend(["--env".into(), "MYSQL_ROOT_PASSWORD".into()]);
    // "mysqladmin ping" succeeds as soon as the server answers, with or without a login. It must go over TCP, like
    // everything the Manager sends: while a new database initialises, the image first runs a temporary server that
    // answers on its socket but takes no network connection, and a socket ping would call the container healthy for a few
    // seconds in which every TCP connection is still refused.
    a.extend(["--health-cmd".into(), "mysqladmin ping --protocol=tcp --host=127.0.0.1 --silent".into()]);
    a.extend(["--health-interval".into(), "5s".into(), "--health-timeout".into(), "5s".into(), "--health-retries".into(), "40".into()]);
    a.extend(["--stop-timeout".into(), "60".into()]);
    a.push(cfg.mysql_image.clone());
    a
}

#[derive(Clone, Copy, PartialEq)]
enum GameKind {
    World,
    Auth,
}

/// Connection strings: host;port;user;password;database. They hold the password, so they travel by environment.
fn database_env(s: &Secrets, world: bool) -> Vec<(String, String)> {
    let info = |db: &str| format!("db;3306;acore;{};{db}", s.app);
    let mut env = vec![("AC_LOGIN_DATABASE_INFO".to_string(), info("acore_auth"))];
    if world {
        env.push(("AC_WORLD_DATABASE_INFO".into(), info("acore_world")));
        env.push(("AC_CHARACTER_DATABASE_INFO".into(), info("acore_characters")));
    }
    env
}

fn game_args(cfg: &Config, n: &Names, kind: GameKind, host: &Path, data: &Path, port: u16, ra_port: u16, owner: Option<&str>) -> Vec<String> {
    let (name, alias, binary, conf, inner_port) = match kind {
        GameKind::World => (&n.world, "world", "./worldserver", "configs/worldserver.conf", WORLD_PORT),
        GameKind::Auth => (&n.auth, "auth", "./authserver", "configs/authserver.conf", AUTH_PORT),
    };
    let mut a: Vec<String> = vec!["run".into(), "--detach".into(), "--name".into(), name.clone()];
    a.extend(["--network".into(), n.network.clone(), "--network-alias".into(), alias.into()]);
    a.extend(["--label".into(), format!("coa.project={}", cfg.project)]);
    if let Some(o) = owner {
        // The server writes logs and configuration into the folder: do it as the person who owns it.
        a.extend(["--user".into(), o.into()]);
    }
    a.extend(["--workdir".into(), CORE.into()]);
    a.extend(["--volume".into(), format!("{}:{CORE}", host.join("Core").display())]);
    a.extend(["--publish".into(), format!("{}:{port}:{inner_port}", cfg.bind_address)]);
    // Settings that must not depend on the configuration files: where things are inside the container, and listening
    // on every interface of the container (the published address decides who can connect).
    let env = |k: &str, v: &str| ["--env".to_string(), format!("{k}={v}")];
    a.extend(env("AC_BIND_IP", "0.0.0.0"));
    a.extend(env(if kind == GameKind::World { "AC_WORLD_SERVER_PORT" } else { "AC_REALM_SERVER_PORT" }, &inner_port.to_string()));
    a.extend(env("AC_LOGS_DIR", &format!("{CORE}/Logs")));
    // The Manager applies database updates itself. The server's own updater would need a `mysql` client program inside
    // the container and stops the server when it cannot find one.
    a.extend(env("AC_UPDATES_ENABLE_DATABASES", "0"));
    if kind == GameKind::World {
        a.extend(["--volume".into(), format!("{}:{DATA}:ro", data.display())]);
        a.extend(env("AC_DATA_DIR", DATA));
        // The remote console is only ever reachable from this computer.
        a.extend(["--publish".into(), format!("127.0.0.1:{ra_port}:{RA_PORT}")]);
        a.extend(env("AC_RA_ENABLE", "1"));
        a.extend(env("AC_RA_IP", "0.0.0.0"));
        a.extend(env("AC_RA_PORT", &RA_PORT.to_string()));
        // Saving every character can take a while.
        a.extend(["--stop-timeout".into(), "180".into()]);
        a.extend(["--env".into(), "AC_WORLD_DATABASE_INFO".into(), "--env".into(), "AC_CHARACTER_DATABASE_INFO".into()]);
    } else {
        a.extend(["--restart".into(), "on-failure:3".into(), "--stop-timeout".into(), "30".into()]);
    }
    a.extend(["--env".into(), "AC_LOGIN_DATABASE_INFO".into()]);
    a.push(runtime_image());
    a.extend([binary.to_string(), "-c".into(), conf.into()]);
    a
}

#[cfg(unix)]
fn owner_of(root: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(root).ok()?;
    Some(format!("{}:{}", m.uid(), m.gid()))
}

#[cfg(not(unix))]
fn owner_of(_root: &Path) -> Option<String> {
    None
}

// ------------------------------------------------------------------------------------------------------------- stop

fn stop(d: &dyn Docker, cfg: &Config, log: &mut Log) -> Step<()> {
    preflight(d, log)?;
    let n = cfg.names();
    let state = inspect(d, &[&n.db, &n.world, &n.auth])?;
    // Game servers first, the database last. The world server gets time to save every character.
    for (name, seconds) in [(&n.world, 180u64), (&n.auth, 30), (&n.db, 60)] {
        if state.get(name).is_some_and(Container::running) {
            let t = seconds.to_string();
            let o = docker(d, &["stop", "--time", &t, name], Duration::from_secs(seconds + 30))?;
            if !o.ok() {
                return fail(ErrorCode::StartupFailed, o.text());
            }
            log.say(format!("stopped {name}"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------- observe

pub fn observe(root: &Path, ports: &Ports) -> Observed {
    observe_with(&SystemDocker, root, ports)
}

pub fn observe_with(d: &dyn Docker, root: &Path, ports: &Ports) -> Observed {
    let unknown = |name: &'static str, port: u16| ServiceStatus { name, state: ServiceState::Unknown, pid: None, port, port_ready: false, conflict: None, uptime_secs: None };
    let Ok(cfg) = Config::load(root) else {
        return Observed { mysql: unknown("mysql", ports.mysql), auth: unknown("auth", ports.auth), world: unknown("world", ports.world), secondary_world: None };
    };
    let n = cfg.names();
    let Ok(state) = inspect(d, &[&n.db, &n.world, &n.auth]) else {
        return Observed { mysql: unknown("mysql", ports.mysql), auth: unknown("auth", ports.auth), world: unknown("world", ports.world), secondary_world: None };
    };
    let ip = connect_ip(&cfg);
    let listening = |c: Option<&Container>, port: u16| c.is_some_and(Container::running) && d.port_open(SocketAddr::new(ip, port));
    Observed {
        mysql: status("mysql", state.get(&n.db), ports.mysql, state.get(&n.db).is_some_and(|c| c.health.as_deref() == Some("healthy"))),
        auth: status("auth", state.get(&n.auth), ports.auth, listening(state.get(&n.auth), ports.auth)),
        world: status("world", state.get(&n.world), ports.world, listening(state.get(&n.world), ports.world)),
        secondary_world: None,
    }
}

fn status(name: &'static str, c: Option<&Container>, port: u16, ready: bool) -> ServiceStatus {
    let Some(c) = c else {
        return ServiceStatus { name, state: ServiceState::Stopped, pid: None, port, port_ready: false, conflict: None, uptime_secs: None };
    };
    let state = match c.status.as_str() {
        "running" if ready => ServiceState::Running,
        "running" | "created" | "restarting" => ServiceState::Starting,
        // Stopped by `docker stop` (SIGTERM, then SIGKILL after the grace period) is a normal stop; anything else is a crash.
        "exited" if c.exit_code == 0 || ((c.exit_code == 137 || c.exit_code == 143) && !c.oom_killed) => ServiceState::Stopped,
        "exited" => ServiceState::Crashed,
        _ => ServiceState::Unknown,
    };
    let running = c.running();
    ServiceStatus {
        name,
        state,
        pid: if running { c.pid } else { None },
        port,
        port_ready: running && ready,
        conflict: None,
        uptime_secs: if running { c.started_at.as_deref().and_then(uptime_since) } else { None },
    }
}

fn uptime_since(started: &str) -> Option<u64> {
    let t = chrono::DateTime::parse_from_rfc3339(started).ok()?;
    u64::try_from((chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::cli::Output;
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashSet};
    use std::fs;

    const ROOT_PW: &str = "rootpw-ZZ1";
    const APP_PW: &str = "apppw-QQ2";

    /// A tiny in-memory Docker: containers, one network, one image. It answers the few commands the backend uses.
    struct Sim {
        calls: RefCell<Vec<Call2>>,
        running: RefCell<HashSet<String>>,
        exited: RefCell<BTreeMap<String, (i64, bool)>>,
        network: RefCell<bool>,
        image: RefCell<bool>,
        logs: RefCell<String>,
        unavailable: bool,
        daemon_error: Option<String>,
        /// `docker run` of this container fails with this message.
        run_error: Option<(String, String)>,
        /// A container that stops right after it was created, with this exit code.
        dies: Option<(String, i64)>,
        port_open: bool,
    }

    #[derive(Clone)]
    struct Call2 {
        args: Vec<String>,
        env: Vec<(String, String)>,
        stdin: bool,
    }

    impl Sim {
        fn new() -> Sim {
            Sim {
                calls: Default::default(),
                running: Default::default(),
                exited: Default::default(),
                network: RefCell::new(false),
                image: RefCell::new(false),
                logs: Default::default(),
                unavailable: false,
                daemon_error: None,
                run_error: None,
                dies: None,
                port_open: true,
            }
        }

        fn verbs(&self) -> Vec<String> {
            self.calls.borrow().iter().map(|c| c.args.iter().take(2).cloned().collect::<Vec<_>>().join(" ")).collect()
        }

        fn calls_of(&self, verb: &str) -> Vec<Call2> {
            self.calls.borrow().iter().filter(|c| c.args[0] == verb).cloned().collect()
        }

        fn inspect_json(&self, names: &[String]) -> (String, bool) {
            let mut items = Vec::new();
            let mut missing = false;
            for n in names {
                if self.running.borrow().contains(n) {
                    let health = if n.ends_with("-db") { r#","Health":{"Status":"healthy"}"# } else { "" };
                    items.push(format!(
                        r#"{{"Name":"/{n}","State":{{"Status":"running","Running":true,"ExitCode":0,"OOMKilled":false,"Pid":4242,"StartedAt":"2026-10-02T12:27:26.892949316Z"{health}}}}}"#
                    ));
                } else if let Some((code, oom)) = self.exited.borrow().get(n) {
                    items.push(format!(
                        r#"{{"Name":"/{n}","State":{{"Status":"exited","Running":false,"ExitCode":{code},"OOMKilled":{oom},"Pid":0,"StartedAt":"2026-10-02T12:27:26.892949316Z"}}}}"#
                    ));
                } else {
                    missing = true;
                }
            }
            (format!("[{}]", items.join(",")), missing)
        }
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> Output {
        Output { code: Some(code), stdout: stdout.into(), stderr: stderr.into() }
    }

    impl Docker for Sim {
        fn run(&self, call: &Call) -> Result<Output> {
            self.calls.borrow_mut().push(Call2 { args: call.args.clone(), env: call.env.clone(), stdin: call.stdin.is_some() });
            if self.unavailable {
                return Err(crate::error::Error::Invalid("docker could not be started: No such file or directory".into()));
            }
            let a: Vec<&str> = call.args.iter().map(String::as_str).collect();
            Ok(match a.as_slice() {
                ["version", ..] => match &self.daemon_error {
                    Some(e) => out(1, "", e),
                    None => out(0, "27.3.1\n", ""),
                },
                ["network", "inspect", ..] => if *self.network.borrow() { out(0, "[]", "") } else { out(1, "[]", "Error: No such network") },
                ["network", "create", ..] => {
                    *self.network.borrow_mut() = true;
                    out(0, "id\n", "")
                }
                ["image", "inspect", ..] => if *self.image.borrow() { out(0, "[]", "") } else { out(1, "[]", "Error: No such image") },
                ["build", ..] => {
                    *self.image.borrow_mut() = true;
                    out(0, "built", "")
                }
                ["inspect", names @ ..] => {
                    let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
                    let (json, missing) = self.inspect_json(&names);
                    if missing { out(1, &json, "Error: No such object: x") } else { out(0, &json, "") }
                }
                ["rm", "-f", name] => {
                    self.running.borrow_mut().remove(*name);
                    self.exited.borrow_mut().remove(*name);
                    out(0, "", "")
                }
                ["run", ..] => {
                    let name = call.args[call.args.iter().position(|x| x == "--name").unwrap() + 1].clone();
                    if let Some((n, e)) = &self.run_error {
                        if *n == name {
                            return Ok(out(125, "", e));
                        }
                    }
                    match &self.dies {
                        Some((n, code)) if *n == name => {
                            self.exited.borrow_mut().insert(name, (*code, false));
                        }
                        _ => {
                            self.running.borrow_mut().insert(name);
                        }
                    }
                    out(0, "containerid\n", "")
                }
                ["stop", "--time", _, name] => {
                    self.running.borrow_mut().remove(*name);
                    self.exited.borrow_mut().insert(name.to_string(), (0, false));
                    out(0, "", "")
                }
                ["logs", ..] => out(0, &self.logs.borrow(), ""),
                other => panic!("unexpected docker call: {other:?}"),
            })
        }

        fn port_open(&self, _addr: SocketAddr) -> bool {
            self.port_open
        }

        fn pause(&self, _d: Duration) {}
    }

    fn server(project: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::write(root.join("Settings/docker.json"), format!(r#"{{"project":"{project}"}}"#)).unwrap();
        fs::write(root.join("Settings/database.json"), format!(r#"{{"rootPassword":"{ROOT_PW}","appPassword":"{APP_PW}"}}"#)).unwrap();
        (d, root)
    }

    fn start_all(sim: &Sim, root: &Path) -> DriverOutcome {
        run_with(sim, root, Verb::StartAll).unwrap()
    }

    #[test]
    fn a_first_start_creates_the_network_the_image_and_the_containers_in_order() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        let o = start_all(&sim, &root);
        assert!(o.ok, "{}", o.output);
        let verbs = sim.verbs();
        let at = |needle: &str| verbs.iter().position(|v| v.starts_with(needle)).unwrap_or_else(|| panic!("{needle} not called: {verbs:?}"));
        assert!(at("version") < at("network create"));
        assert!(at("network create") < at("run --detach"), "database comes first");
        let runs: Vec<String> = sim.calls_of("run").iter().map(|c| c.args[c.args.iter().position(|x| x == "--name").unwrap() + 1].clone()).collect();
        assert_eq!(runs, ["coa-t1-db", "coa-t1-world", "coa-t1-auth"], "database, then world, then auth");
        assert!(at("build") < verbs.iter().rposition(|v| v.starts_with("run")).unwrap(), "the runtime image exists before the game servers");
        assert!(sim.calls_of("build")[0].stdin, "the Dockerfile travels on stdin");
    }

    #[test]
    fn passwords_travel_by_environment_and_never_on_a_command_line() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        assert!(start_all(&sim, &root).ok);
        for c in sim.calls.borrow().iter() {
            for arg in &c.args {
                assert!(!arg.contains(ROOT_PW) && !arg.contains(APP_PW), "secret on a command line: {arg}");
            }
        }
        let runs = sim.calls_of("run");
        assert!(runs[0].env.contains(&("MYSQL_ROOT_PASSWORD".into(), ROOT_PW.into())));
        assert!(runs[1].env.iter().any(|(k, v)| k == "AC_LOGIN_DATABASE_INFO" && v == &format!("db;3306;acore;{APP_PW};acore_auth")));
        assert!(runs[1].env.iter().any(|(k, _)| k == "AC_CHARACTER_DATABASE_INFO"));
        assert!(runs[2].env.iter().any(|(k, _)| k == "AC_LOGIN_DATABASE_INFO"));
        assert!(!runs[2].env.iter().any(|(k, _)| k == "AC_WORLD_DATABASE_INFO"), "auth does not need the world database");
        assert!(runs[1].args.windows(2).any(|w| w == ["--env", "AC_WORLD_DATABASE_INFO"]), "name only, the value is in the environment");
    }

    #[test]
    fn the_database_is_only_healthy_once_it_takes_tcp_connections() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        assert!(start_all(&sim, &root).ok);
        let db = &sim.calls_of("run")[0].args;
        let i = db.iter().position(|a| a == "--health-cmd").expect("the database has a health check");
        // Found on a new volume: a socket-only ping reported healthy about four seconds before TCP worked.
        assert!(db[i + 1].contains("--protocol=tcp") && db[i + 1].contains("--host=127.0.0.1"), "{}", db[i + 1]);
    }

    #[test]
    fn the_world_container_is_wired_like_the_repack_layout() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        assert!(start_all(&sim, &root).ok);
        let world = &sim.calls_of("run")[1].args;
        let has = |s: &str| world.iter().any(|a| a == s);
        assert!(has("127.0.0.1:8085:8085"), "game port on this computer only by default");
        assert!(has("127.0.0.1:3443:3443"), "the remote console is never exposed beyond this computer");
        assert!(has("AC_BIND_IP=0.0.0.0") && has("AC_RA_ENABLE=1") && has("AC_RA_IP=0.0.0.0"));
        assert!(has("AC_UPDATES_ENABLE_DATABASES=0"), "the server's own database updater needs a mysql client the image does not have");
        assert!(sim.calls_of("run")[2].args.iter().any(|a| a == "AC_UPDATES_ENABLE_DATABASES=0"));
        assert!(has("AC_DATA_DIR=/srv/data") && has("AC_LOGS_DIR=/srv/core/Logs"));
        assert!(world.iter().any(|a| a.ends_with(":/srv/data:ro")), "game data is read-only");
        assert!(world.iter().any(|a| a.ends_with(":/srv/core") && a.contains("Core")));
        assert!(world.windows(2).any(|w| w == ["--workdir", "/srv/core"]));
        assert!(world.windows(2).any(|w| w == ["--stop-timeout", "180"]));
        let n = world.len();
        assert_eq!(&world[n - 3..], ["./worldserver", "-c", "configs/worldserver.conf"]);
        assert!(world[n - 4].starts_with("coa-runtime:"));
        assert!(root.join("Core/Logs").is_dir(), "created by the Manager so it belongs to the user");
    }

    #[test]
    fn the_published_address_follows_the_setting() {
        let (_d, root) = server("t1");
        fs::write(root.join("Settings/docker.json"), r#"{"project":"t1","bindAddress":"0.0.0.0"}"#).unwrap();
        let sim = Sim::new();
        assert!(start_all(&sim, &root).ok);
        let runs = sim.calls_of("run");
        assert!(runs[1].args.iter().any(|a| a == "0.0.0.0:8085:8085"));
        assert!(runs[1].args.iter().any(|a| a == "127.0.0.1:3443:3443"), "the console stays private");
        assert!(runs[2].args.iter().any(|a| a == "0.0.0.0:3724:3724"));
    }

    #[test]
    fn custom_host_ports_keep_the_container_listeners_on_the_mapped_ports() {
        let (_d, root) = server("ports");
        let cfg = Config::load(&root).unwrap();
        let n = cfg.names();
        let world = game_args(&cfg, &n, GameKind::World, &root, &root.join("Data"), 18085, 13443, None);
        let auth = game_args(&cfg, &n, GameKind::Auth, &root, &root.join("Data"), 13724, 0, None);
        assert!(world.iter().any(|a| a == "127.0.0.1:18085:8085"));
        assert!(world.iter().any(|a| a == "127.0.0.1:13443:3443"));
        assert!(world.iter().any(|a| a == "AC_WORLD_SERVER_PORT=8085"));
        assert!(world.iter().any(|a| a == "AC_RA_PORT=3443"));
        assert!(auth.iter().any(|a| a == "127.0.0.1:13724:3724"));
        assert!(auth.iter().any(|a| a == "AC_REALM_SERVER_PORT=3724"));
    }

    #[test]
    fn a_second_start_leaves_running_containers_alone() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        assert!(start_all(&sim, &root).ok);
        let before = sim.calls_of("run").len();
        assert!(start_all(&sim, &root).ok);
        assert_eq!(sim.calls_of("run").len(), before, "nothing was recreated");
    }

    #[test]
    fn start_database_only_does_not_touch_the_game_servers() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        assert!(run_with(&sim, &root, Verb::StartMysql).unwrap().ok);
        assert_eq!(sim.calls_of("run").len(), 1);
        assert!(sim.calls_of("build").is_empty());
    }

    #[test]
    fn secondary_world_start_is_rejected_without_touching_containers() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        assert!(run_with(&sim, &root, Verb::StartWorld).is_err());
        assert!(sim.calls.borrow().is_empty());
    }

    #[test]
    fn stop_goes_world_then_auth_then_database_with_time_to_save() {
        let (_d, root) = server("t1");
        let sim = Sim::new();
        assert!(start_all(&sim, &root).ok);
        sim.calls.borrow_mut().clear();
        let o = run_with(&sim, &root, Verb::StopAll).unwrap();
        assert!(o.ok, "{}", o.output);
        let stops: Vec<(String, String)> = sim.calls_of("stop").iter().map(|c| (c.args[3].clone(), c.args[2].clone())).collect();
        assert_eq!(stops, [("coa-t1-world".into(), "180".into()), ("coa-t1-auth".into(), "30".into()), ("coa-t1-db".into(), "60".into())]);
        assert!(sim.running.borrow().is_empty());
        // Stopping what is already stopped is fine.
        sim.calls.borrow_mut().clear();
        assert!(run_with(&sim, &root, Verb::StopAll).unwrap().ok);
        assert!(sim.calls_of("stop").is_empty());
    }

    #[test]
    fn docker_not_installed_is_reported_as_unavailable() {
        let (_d, root) = server("t1");
        let mut sim = Sim::new();
        sim.unavailable = true;
        let o = start_all(&sim, &root);
        assert!(!o.ok);
        assert_eq!(o.code, Some(ErrorCode::DockerUnavailable));
        assert!(o.output.contains("could not be started"));
    }

    #[test]
    fn a_refused_connection_to_the_docker_service_shows_docker_own_words() {
        let (_d, root) = server("t1");
        let mut sim = Sim::new();
        sim.daemon_error = Some("permission denied while trying to connect to the Docker daemon socket".into());
        let o = start_all(&sim, &root);
        assert_eq!(o.code, Some(ErrorCode::DockerUnavailable));
        assert!(o.output.contains("permission denied"), "{}", o.output);
        assert!(sim.calls_of("run").is_empty(), "nothing is started when docker is not usable");
    }

    #[test]
    fn a_world_server_that_stops_while_starting_reports_the_cause_found_in_its_log() {
        let (_d, root) = server("t1");
        let mut sim = Sim::new();
        sim.dies = Some(("coa-t1-world".into(), 1));
        *sim.logs.borrow_mut() = "\u{1b}[31mCould not connect to MySQL database at db: Can't connect to MySQL server on 'db:3306' (111)\u{1b}[0m".into();
        let o = start_all(&sim, &root);
        assert!(!o.ok);
        assert_eq!(o.code, Some(ErrorCode::DatabaseNotRunning));
        assert!(o.output.contains("Can't connect to MySQL server"), "{}", o.output);
        assert!(!o.output.contains('\u{1b}'), "colour codes are removed");
        assert!(sim.calls_of("run").len() == 2, "auth is not started after a failed world");
    }

    #[test]
    fn a_port_that_is_taken_is_reported_as_such() {
        let (_d, root) = server("t1");
        let mut sim = Sim::new();
        sim.run_error = Some(("coa-t1-world".into(), "Bind for 127.0.0.1:8085 failed: port is already allocated".into()));
        let o = start_all(&sim, &root);
        assert_eq!(o.code, Some(ErrorCode::PortInUse));
    }

    #[test]
    fn a_server_that_never_listens_times_out_with_its_log() {
        let (_d, root) = server("t1");
        let mut sim = Sim::new();
        sim.port_open = false;
        *sim.logs.borrow_mut() = "still loading".into();
        let o = start_all(&sim, &root);
        assert!(!o.ok);
        assert!(o.output.contains("was not listening on port 8085") && o.output.contains("still loading"), "{}", o.output);
    }

    #[test]
    fn invalid_settings_are_refused_before_anything_runs() {
        for (project, bind, image) in [("Bad Name", "127.0.0.1", "mysql:8.4"), ("", "127.0.0.1", "mysql:8.4"), ("ok", "not-an-ip", "mysql:8.4"), ("ok", "127.0.0.1", "mysql:8.4; rm -rf /"), ("-lead", "127.0.0.1", "mysql:8.4")] {
            let (_d, root) = server("t1");
            fs::write(root.join("Settings/docker.json"), format!(r#"{{"project":"{project}","bindAddress":"{bind}","mysqlImage":"{image}"}}"#)).unwrap();
            let sim = Sim::new();
            assert!(run_with(&sim, &root, Verb::StartAll).is_err(), "{project} / {bind} / {image}");
            assert!(sim.calls.borrow().is_empty());
        }
    }

    #[test]
    fn inspect_output_is_read_like_docker_prints_it() {
        // Shape taken from `docker inspect` on a stopped database container and a world server.
        let json = r#"[{"Id":"abc","Name":"/coa-t1-db","State":{"Status":"running","Running":true,"Paused":false,"Restarting":false,"OOMKilled":false,"Dead":false,"Pid":2402,"ExitCode":0,"Error":"","StartedAt":"2026-10-02T12:27:26.892949316Z","FinishedAt":"0001-01-01T00:00:00Z","Health":{"Status":"healthy","FailingStreak":0,"Log":[]}}},
                       {"Name":"/coa-t1-world","State":{"Status":"exited","Running":false,"OOMKilled":false,"Pid":0,"ExitCode":137,"StartedAt":"2026-10-02T12:40:06.76552263Z"}}]"#;
        let m = parse_inspect(json);
        assert_eq!(m["coa-t1-db"].pid, Some(2402));
        assert_eq!(m["coa-t1-db"].health.as_deref(), Some("healthy"));
        assert_eq!(m["coa-t1-world"].exit_code, 137);
        assert_eq!(m["coa-t1-world"].pid, None);
        assert!(parse_inspect("[]").is_empty() && parse_inspect("").is_empty() && parse_inspect("nonsense").is_empty());
    }

    #[test]
    fn observation_maps_container_states_to_service_states() {
        let ports = Ports::default();
        let (_d, root) = server("t1");

        // Nothing created yet.
        let sim = Sim::new();
        let o = observe_with(&sim, &root, &ports);
        assert!([o.mysql.state, o.auth.state, o.world.state].iter().all(|s| *s == ServiceState::Stopped));

        // Everything up.
        assert!(start_all(&sim, &root).ok);
        let o = observe_with(&sim, &root, &ports);
        assert_eq!((o.mysql.state, o.auth.state, o.world.state), (ServiceState::Running, ServiceState::Running, ServiceState::Running));
        assert!(o.world.port_ready && o.world.pid == Some(4242) && o.world.uptime_secs.is_some());
        assert_eq!(o.world.port, ports.world);

        // Running but not listening yet.
        let mut slow = Sim::new();
        slow.port_open = false;
        slow.running.borrow_mut().insert("coa-t1-world".into());
        let o = observe_with(&slow, &root, &ports);
        assert_eq!(o.world.state, ServiceState::Starting);
        assert!(!o.world.port_ready);

        // Stopped cleanly, killed after the grace period, crashed, killed for lack of memory.
        let mut sim = Sim::new();
        for (name, code, oom) in [("coa-t1-db", 0, false), ("coa-t1-auth", 137, false), ("coa-t1-world", 1, false)] {
            sim.exited.borrow_mut().insert(name.into(), (code, oom));
        }
        let o = observe_with(&sim, &root, &ports);
        assert_eq!((o.mysql.state, o.auth.state, o.world.state), (ServiceState::Stopped, ServiceState::Stopped, ServiceState::Crashed));
        sim.exited.borrow_mut().insert("coa-t1-auth".into(), (137, true));
        assert_eq!(observe_with(&sim, &root, &ports).auth.state, ServiceState::Crashed);
        sim.unavailable = true;
        assert_eq!(observe_with(&sim, &root, &ports).world.state, ServiceState::Unknown, "docker unreachable: unknown, not stopped");
    }

    #[test]
    fn the_runtime_image_is_named_after_its_dockerfile() {
        let name = runtime_image();
        assert!(name.starts_with("coa-runtime:") && name.len() == "coa-runtime:".len() + 12, "{name}");
        assert!(RUNTIME_DOCKERFILE.contains("libmysqlclient24") && RUNTIME_DOCKERFILE.contains("libreadline8t64"));
    }

    #[test]
    fn a_docker_folder_is_scanned_without_the_repack_files() {
        let (_d, root) = server("t1");
        assert!(crate::docker::is_docker(&root));
        for f in ["Core/worldserver", "Core/authserver", "Core/configs/worldserver.conf", "Core/configs/authserver.conf", "Data/dbc/x", "Data/maps/x"] {
            let p = root.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
        let r = crate::layout::scan(&root).unwrap();
        assert_eq!(r.classification, crate::layout::Classification::Healthy);
        assert!(r.worldserver.is_some() && r.authserver.is_some() && !r.modifies_files);
        fs::remove_file(root.join("Core/authserver")).unwrap();
        assert_eq!(crate::layout::scan(&root).unwrap().classification, crate::layout::Classification::Partial);
        fs::remove_file(root.join("Settings/docker.json")).unwrap();
        assert!(!crate::docker::is_docker(&root), "without the marker it is scanned as a repack");
    }
}
