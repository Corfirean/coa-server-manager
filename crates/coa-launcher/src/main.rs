//! `coa-launcher.exe`: the repack launcher, the same commands as `Scripts/manage.py`.
//!
//! It lives in `<server>/Scripts/` next to manage.py (the server folder is its parent), or takes `--root <folder>`.
//! Errors are printed as `ERROR: <message>` on stderr with exit code 1, exactly like manage.py, because the
//! Manager reads them.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use coa_core::launcher::{render, services, settings::Settings, state, world};

const ACTIONS: [&str; 17] = [
    "start-all", "start-mysql", "start-auth", "start-world", "stop-all", "status", "status-json", "create-account",
    "create-account-once", "console", "ra-once", "prepare", "watch-world", "relay-worker", "debug-logs-on",
    "debug-logs-off", "debug-logs-status",
];

fn say(text: &str) {
    println!("{text}");
    let _ = std::io::stdout().flush();
}

fn usage() -> String {
    format!("usage: coa-launcher [--root <server>] [--port-offset N] {{{}}}", ACTIONS.join(","))
}

struct Args {
    root: PathBuf,
    offset: i32,
    action: String,
}

fn parse() -> Result<Args, String> {
    let mut root: Option<PathBuf> = None;
    let mut offset = 0;
    let mut action = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--root" => root = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            "--port-offset" => offset = it.next().and_then(|v| v.parse().ok()).ok_or_else(usage)?,
            _ if ACTIONS.contains(&a.as_str()) && action.is_none() => action = Some(a),
            _ => return Err(usage()),
        }
    }
    let root = match root {
        Some(r) => r,
        None => {
            // <server>/Scripts/coa-launcher.exe -> <server>
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            exe.parent().and_then(Path::parent).map(Path::to_path_buf).ok_or("cannot find the server folder")?
        }
    };
    Ok(Args { root, offset, action: action.ok_or_else(usage)? })
}

fn read_line(prompt: &str) -> std::io::Result<String> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// Read a line without showing it (passwords).
fn read_secret(prompt: &str) -> std::io::Result<String> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE};
        let h = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode = 0;
        if GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleMode(h, mode & !ENABLE_ECHO_INPUT);
            let line = read_line(prompt);
            SetConsoleMode(h, mode);
            println!();
            return line;
        }
    }
    read_line(prompt)
}

fn valid_login(text: &str) -> bool {
    (1..=16).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn ra(cfg: &Settings, root: &Path, command: &str) -> Result<String, String> {
    if !services::ready(root, "world", cfg.ra_port) {
        return Err("Start this repack's worldserver first.".into());
    }
    let mut console = coa_core::ra::Ra::connect_to(cfg.ra_port, &cfg.ra_username, &cfg.ra_password).map_err(|e| e.to_string())?;
    console.run(command).map_err(|e| e.to_string())
}

fn run(args: Args) -> Result<(), String> {
    let root = args.root.as_path();
    let e = |x: coa_core::Error| x.to_string();
    match args.action.as_str() {
        "watch-world" => return world::watch_world(root, &|m| say(m)).map_err(e),
        "relay-worker" => {
            return Err("The bug-report relay runs through Scripts/manage.py relay-worker until it is ported.".into())
        }
        "debug-logs-status" => {
            let on = services::status(root)["debugLogs"].as_bool().unwrap_or(false);
            say(&format!("Detailed server logging is {}.", if on { "ON" } else { "off" }));
            if on {
                say("Every AzerothCore logger is forced to Trace level in Core/Logs (Server.log, Auth.log, world-console.log, auth-console.log). Expect much larger, faster-growing log files.");
            }
            say("Run Debug_Logs_On.bat / Debug_Logs_Off.bat, then restart the affected service(s) to apply.");
            return Ok(());
        }
        "debug-logs-on" | "debug-logs-off" => {
            let on = args.action == "debug-logs-on";
            let changed = {
                let _lock = state::ControlLock::acquire(root, &args.action, Duration::from_secs(30), &|m| say(m)).map_err(e)?;
                world::debug_logs(root, on).map_err(e)?
            };
            if changed.is_empty() {
                say(&format!("Detailed logging was already {}.", if on { "on" } else { "off" }));
            } else {
                say(&format!("{} detailed logging in: {}", if on { "Enabled" } else { "Restored" }, changed.join(", ")));
            }
            say("Restart the affected service(s) (Stop_All_Server.bat then Start_All_Server.bat) to apply this.");
            return Ok(());
        }
        _ => {}
    }
    let cfg = Settings::load(root, args.offset).map_err(e)?;
    match args.action.as_str() {
        "console" => {
            say("World console. Enter a server command, or exit to close this window.");
            loop {
                let command = read_line("AC> ").map_err(|x| x.to_string())?;
                let command = command.trim();
                if command.eq_ignore_ascii_case("exit") || command.eq_ignore_ascii_case("quit") {
                    return Ok(());
                }
                if !command.is_empty() {
                    say(&ra(&cfg, root, command)?);
                }
            }
        }
        "create-account" => {
            let name = read_line("New account name (letters and digits, up to 16): ").map_err(|x| x.to_string())?;
            let name = name.trim();
            let password = read_secret("New account password (letters and digits, up to 16): ").map_err(|x| x.to_string())?;
            let again = read_secret("Repeat password: ").map_err(|x| x.to_string())?;
            if !valid_login(name) || !valid_login(&password) {
                return Err("Use 1-16 letters or digits for the account name and password.".into());
            }
            if password != again {
                return Err("Passwords do not match.".into());
            }
            say(&ra(&cfg, root, &format!("account create {name} {password}"))?);
            if read_line("Give this account GM permissions? [y/N]: ").map_err(|x| x.to_string())?.trim().eq_ignore_ascii_case("y") {
                say(&ra(&cfg, root, &format!("account set gmlevel {name} 3 -1"))?);
            }
            return Ok(());
        }
        "ra-once" => {
            let command = read_line("").map_err(|x| x.to_string())?;
            let command = command.trim();
            if command.is_empty() {
                return Err("No command given.".into());
            }
            say(&ra(&cfg, root, command)?);
            return Ok(());
        }
        "create-account-once" => {
            let line = read_line("").map_err(|x| x.to_string())?;
            let request: serde_json::Value = serde_json::from_str(&line).map_err(|x| x.to_string())?;
            let name = request.get("username").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let password = request.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !valid_login(&name) || !valid_login(&password) {
                return Err("Use 1-16 letters or digits for the account name and password.".into());
            }
            let mut messages = vec![ra(&cfg, root, &format!("account create {name} {password}"))?];
            if request.get("gm").and_then(|v| v.as_bool()).unwrap_or(false) {
                messages.push(ra(&cfg, root, &format!("account set gmlevel {name} 3 -1"))?);
            }
            say(&serde_json::json!({ "messages": messages }).to_string());
            return Ok(());
        }
        _ => {}
    }
    {
        let _lock = state::ControlLock::acquire(root, &args.action, Duration::from_secs(30), &|m| say(m)).map_err(e)?;
        match args.action.as_str() {
            "status" => {
                for name in ["mysql", "auth", "world", "relay"] {
                    let line = match state::running(root, name) {
                        Some(r) => format!("{name:12} running (PID {})", r.pid),
                        None => format!("{name:12} stopped"),
                    };
                    say(&line);
                }
                return Ok(());
            }
            "status-json" => {
                say(&services::status(root).to_string());
                return Ok(());
            }
            "stop-all" => return services::stop_all(root, &cfg, &|m| say(m)).map_err(e),
            _ => {}
        }
        if matches!(args.action.as_str(), "start-all" | "start-world") {
            coa_core::launcher::squid::validate_bots(root).map_err(e)?;
        }
        render::prepare(root, &cfg).map_err(e)?;
        if args.action == "prepare" {
            return Ok(());
        }
        let progress = |m: &str| say(m);
        if matches!(args.action.as_str(), "start-all" | "start-mysql") {
            services::start_mysql(root, &cfg, &progress).map_err(e)?;
        }
        if matches!(args.action.as_str(), "start-all" | "start-auth") {
            services::start_auth(root, &cfg, &progress).map_err(e)?;
        }
        if matches!(args.action.as_str(), "start-all" | "start-world") {
            let me = std::env::current_exe().map_err(|x| x.to_string())?;
            world::start_world(root, &cfg, args.offset, &me, &progress).map_err(e)?;
        }
    }
    if args.action.starts_with("start-") {
        say("Use Stop_All_Server.bat to shut down this repack safely.");
    }
    Ok(())
}

fn main() {
    let result = parse().and_then(run);
    if let Err(message) = result {
        eprintln!("ERROR: {message}");
        std::process::exit(1);
    }
}
