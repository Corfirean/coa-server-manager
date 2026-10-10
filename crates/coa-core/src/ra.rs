//! Minimal Remote Access (RA) console client for the worldserver, local only.
//! Only a fixed set of typed operations is exposed; free-form commands are not accepted here.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fsx;
use crate::layout::read_ports;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RaSettings {
    ra_username: String,
    ra_password: String,
}

#[derive(Debug)]
pub struct Ra {
    stream: TcpStream,
}

fn read_until(stream: &mut TcpStream, marker: &[u8]) -> Result<String> {
    let mut data = Vec::new();
    let mut byte = [0u8; 1];
    while !data.ends_with(marker) {
        match stream.read(&mut byte) {
            Ok(0) => return Err(Error::Invalid("The world console closed the connection.".into())),
            Ok(_) => data.push(byte[0]),
            Err(e) => return Err(Error::Invalid(format!("The world console did not answer: {e}"))),
        }
        if data.len() > 256 * 1024 {
            return Err(Error::Invalid("The world console sent too much data.".into()));
        }
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

impl Ra {
    /// Connect to the loopback RA port of the server in `root`, using the credentials from its `repack.json`.
    pub fn connect(root: &Path) -> Result<Ra> {
        let cfg: RaSettings = fsx::read_json(&root.join("Settings/repack.json")).map_err(|_| Error::Invalid("The server console settings could not be read.".into()))?;
        Self::connect_to(read_ports(root).ra, &cfg.ra_username, &cfg.ra_password)
    }

    pub fn connect_to(port: u16, user: &str, password: &str) -> Result<Ra> {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|_| Error::Invalid("The world server is not running (its console is not reachable).".into()))?;
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        read_until(&mut stream, b"Username: ")?;
        stream.write_all(format!("{user}\r\n").as_bytes())?;
        read_until(&mut stream, b"Password: ")?;
        stream.write_all(format!("{password}\r\n").as_bytes())?;
        let banner = read_until(&mut stream, b"AC>").map_err(|_| Error::Invalid("The world console rejected the login.".into()))?;
        if banner.to_lowercase().contains("authentication failed") {
            return Err(Error::Invalid("The world console rejected the login.".into()));
        }
        Ok(Ra { stream })
    }

    /// Send one already-validated console command and return its output.
    pub fn run(&mut self, cmd: &str) -> Result<String> {
        let c = crate::console::check_command(cmd)?;
        self.command(c)
    }

    fn command(&mut self, cmd: &str) -> Result<String> {
        debug_assert!(!cmd.contains(['\r', '\n']));
        self.stream.write_all(format!("{cmd}\r\n").as_bytes())?;
        let out = read_until(&mut self.stream, b"AC>")?;
        Ok(out.trim_end_matches("AC>").trim().to_string())
    }

    pub fn create_account(&mut self, name: &str, password: &str) -> Result<()> {
        validate_account(name, password)?;
        let out = self.command(&format!("account create {name} {password}"))?;
        let l = out.to_lowercase();
        if l.contains("already exist") {
            return Err(Error::Invalid("That account name is already taken.".into()));
        }
        if l.contains("created") {
            Ok(())
        } else {
            Err(Error::Invalid(format!("The server did not create the account: {}", out.lines().last().unwrap_or(""))))
        }
    }

    /// World update timing from the server's own `server info` report (mean/median/percentiles of the last 500 updates).
    pub fn performance(&mut self) -> Result<Option<Performance>> {
        let out = self.command("server info")?;
        Ok(parse_server_info(&out))
    }

    fn bot_command(&mut self, cmd: &str) -> Result<String> {
        let out = self.command(cmd)?;
        if is_unsupported_command(&out) {
            return Err(Error::CompanionCommandUnsupported);
        }
        Ok(out)
    }

    /// Drop every bot still waiting to be created. Returns how many were waiting.
    pub fn cancel_spawning(&mut self) -> Result<u32> {
        Ok(first_number(&self.bot_command("botcmd spawncancel")?))
    }

    /// Log every bot out without deleting anything. Returns how many were online.
    pub fn despawn_all(&mut self) -> Result<u32> {
        Ok(first_number(&self.bot_command("botcmd despawnall")?))
    }

    /// Log one bot out (the character stays saved). False when that bot was not online.
    pub fn despawn_bot(&mut self, guid: u64) -> Result<bool> {
        Ok(self.bot_command(&format!("botcmd despawn {guid}"))?.to_lowercase().contains("despawned"))
    }

    /// Log every bot out and delete every bot character for good (the caller saves a recovery point first).
    pub fn purge_all(&mut self) -> Result<String> {
        let out = self.bot_command("botcmd purgeall")?;
        Ok(out.lines().last().unwrap_or("").to_string())
    }

    /// Ask the bot module to create `count` leveling bots (throttled by the module itself). Only a number is sent.
    pub fn spawn_bots(&mut self, count: u32) -> Result<String> {
        if !(1..=2000).contains(&count) {
            return Err(Error::Invalid("Choose between 1 and 2000 companions.".into()));
        }
        let out = self.command(&format!("botcmd spawnleveled {count}"))?;
        if out.to_lowercase().contains("no usable template characters") {
            return Err(Error::CompanionTemplatesMissing);
        }
        Ok(out.lines().last().unwrap_or("").to_string())
    }

    /// Set a new password for an existing account.
    pub fn set_account_password(&mut self, name: &str, password: &str) -> Result<()> {
        validate_account(name, password)?;
        let out = self.command(&format!("account set password {name} {password} {password}"))?;
        account_reply(&out, "The password was not changed")
    }

    /// Access level of an account on all realms: 0 player, 1 moderator, 2 game master, 3 administrator.
    pub fn set_account_access(&mut self, name: &str, level: u8) -> Result<()> {
        validate_account(name, "placeholder")?;
        if level > 3 {
            return Err(Error::Invalid("The access level must be 0 to 3.".into()));
        }
        let out = self.command(&format!("account set gmlevel {name} {level} -1"))?;
        account_reply(&out, "The access level was not changed")
    }

    /// Delete an account together with its characters (the console's `account delete`).
    pub fn delete_account(&mut self, name: &str) -> Result<()> {
        validate_account(name, "placeholder")?;
        let out = self.command(&format!("account delete {name}"))?;
        account_reply(&out, "The account was not deleted")
    }

    /// Give `name` administrator rights on all realms (GM level 3).
    pub fn make_administrator(&mut self, name: &str) -> Result<()> {
        validate_account(name, "placeholder")?;
        let out = self.command(&format!("account set gmlevel {name} 3 -1"))?;
        if out.to_lowercase().contains("gm level") || out.to_lowercase().contains("security") || out.is_empty() {
            Ok(())
        } else {
            Err(Error::Invalid(format!("Could not set administrator rights: {out}")))
        }
    }
}

/// The console reports a failed account command in words; anything else (including silence) counts as done.
fn account_reply(out: &str, what: &str) -> Result<()> {
    let l = out.to_lowercase();
    let failed = ["does not exist", "not exist", "not found", "do not match", "don't match", "usage", "unknown", "incorrect", "error"];
    if failed.iter().any(|m| l.contains(m)) {
        return Err(Error::Invalid(format!("{what}: {}", out.lines().last().unwrap_or("").trim())));
    }
    Ok(())
}

/// The console answers an unknown sub-command with the list of the ones it knows.
fn is_unsupported_command(out: &str) -> bool {
    let l = out.to_lowercase();
    l.contains("possible subcommands") || l.contains("### usage") || l.contains("no such command") || l.contains("unknown command")
}

fn first_number(text: &str) -> u32 {
    text.split(|c: char| !c.is_ascii_digit()).find(|t| !t.is_empty()).and_then(|t| t.parse().ok()).unwrap_or(0)
}

/// How fast the world loop is running, read from `server info`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Performance {
    pub mean_ms: u32,
    pub median_ms: u32,
    pub p95_ms: u32,
    pub p99_ms: u32,
    pub max_ms: u32,
    /// World updates per second implied by the mean update time.
    pub ticks_per_sec: f32,
}

fn ms(text: &str) -> Option<u32> {
    text.trim().trim_end_matches("ms").trim().parse().ok()
}

pub fn parse_server_info(text: &str) -> Option<Performance> {
    let (mut mean, mut median, mut pct) = (None, None, None);
    for line in text.lines() {
        let line = line.trim().trim_start_matches('|').trim_start_matches('-').trim();
        if let Some(v) = line.strip_prefix("Mean:") {
            mean = ms(v);
        } else if let Some(v) = line.strip_prefix("Median:") {
            median = ms(v);
        } else if let Some(v) = line.strip_prefix("Percentiles (95, 99, max):") {
            let p: Vec<Option<u32>> = v.split(',').map(ms).collect();
            if p.len() == 3 {
                pct = Some((p[0]?, p[1]?, p[2]?));
            }
        }
    }
    let (mean_ms, median_ms, (p95_ms, p99_ms, max_ms)) = (mean?, median?, pct?);
    Some(Performance { mean_ms, median_ms, p95_ms, p99_ms, max_ms, ticks_per_sec: 1000.0 / mean_ms.max(1) as f32 })
}

/// Account names are letters/digits (3-17); passwords are 6-16 printable characters without spaces or quotes.
pub fn validate_account(name: &str, password: &str) -> Result<()> {
    let bad = |m: &str| Err(Error::Invalid(m.into()));
    if !(3..=17).contains(&name.len()) || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return bad("The username must be 3–17 letters or digits.");
    }
    if !(6..=16).contains(&password.len()) || !password.chars().all(|c| c.is_ascii_graphic() && c != '"' && c != '\'') {
        return bad("The password must be 6–16 characters (no spaces or quotes).");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    /// These tests share the loopback interface; on some machines (security software hooking TCP) several
    /// concurrent short-lived loopback connections are occasionally reset, so they run one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fake_ra(reply: &'static str) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut got = Vec::new();
            s.write_all(b"Username: ").unwrap();
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            got.push(line.trim().to_string());
            s.write_all(b"Password: ").unwrap();
            line.clear();
            r.read_line(&mut line).unwrap();
            got.push(line.trim().to_string());
            s.write_all(b"Authentication Succeeded\r\nAC>").unwrap();
            line.clear();
            if r.read_line(&mut line).unwrap_or(0) > 0 {
                got.push(line.trim().to_string());
                s.write_all(format!("{reply}\r\nAC>").as_bytes()).unwrap();
            }
            // Close gracefully: dropping a socket with unread data would send RST and could discard the reply.
            let _ = s.shutdown(std::net::Shutdown::Write);
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut sink = [0u8; 64];
            while matches!(s.read(&mut sink), Ok(n) if n > 0) {}
            got
        });
        (port, h)
    }

    #[test]
    fn an_unknown_bot_subcommand_is_recognised_and_numbers_are_read_from_replies() {
        assert!(is_unsupported_command("### USAGE: .botcmd ...
Possible subcommands:
|- botcmd despawn"));
        assert!(!is_unsupported_command("BotMgr: cancelled 35 queued bot spawn(s)."));
        assert_eq!(first_number("BotMgr: cancelled 35 queued bot spawn(s)."), 35);
        assert_eq!(first_number("nothing"), 0);
    }

    #[test]
    fn server_info_timing_is_parsed_and_unrelated_text_is_ignored() {
        let text = "AzerothCore rev. x
Connected players: 0. Characters in world: 0.
Update time diff: 1ms. Last 500 diffs summary:
|- Mean: 14ms
|- Median: 15ms
|- Percentiles (95, 99, max): 24ms, 30ms, 61ms
AC>";
        let p = parse_server_info(text).unwrap();
        assert_eq!((p.mean_ms, p.median_ms, p.p95_ms, p.p99_ms, p.max_ms), (14, 15, 24, 30, 61));
        assert!((p.ticks_per_sec - 71.4).abs() < 0.2);
        assert!(parse_server_info("Connected players: 0.").is_none());
    }

    #[test]
    fn creates_an_account_over_ra_with_the_expected_commands() {
        let _lock = serial();
        let (port, h) = fake_ra("Account created: PLAYER1");
        let mut ra = Ra::connect_to(port, "local", "secretra").unwrap();
        ra.create_account("Player1", "hunter22").unwrap();
        drop(ra);
        assert_eq!(h.join().unwrap(), ["local", "secretra", "account create Player1 hunter22"]);
    }

    #[test]
    fn changes_a_password_with_the_expected_command() {
        let _lock = serial();
        let (port, h) = fake_ra("The password was changed");
        let mut ra = Ra::connect_to(port, "u", "p").unwrap();
        ra.set_account_password("Player1", "newpass9").unwrap();
        drop(ra);
        assert_eq!(h.join().unwrap()[2], "account set password Player1 newpass9 newpass9");
    }

    #[test]
    fn changes_an_access_level_with_the_expected_command() {
        let _lock = serial();
        let (port, h) = fake_ra("You have changed security level of Player1 to 2.");
        let mut ra = Ra::connect_to(port, "u", "p").unwrap();
        ra.set_account_access("Player1", 2).unwrap();
        assert!(ra.set_account_access("Player1", 4).is_err(), "only 0 to 3");
        drop(ra);
        assert_eq!(h.join().unwrap()[2], "account set gmlevel Player1 2 -1");
    }

    #[test]
    fn a_failed_account_command_is_reported_and_bad_names_never_reach_the_console() {
        let _lock = serial();
        let (port, _h) = fake_ra("Account not found.");
        let mut ra = Ra::connect_to(port, "u", "p").unwrap();
        assert!(ra.set_account_password("x", "newpass9").is_err(), "the name is validated before anything is sent");
        assert!(ra.set_account_password("Nobody", "newpass9").unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn spawn_bots_sends_only_a_bounded_number() {
        let _lock = serial();
        let (port, h) = fake_ra("Queued 50 bots");
        let mut ra = Ra::connect_to(port, "u", "p").unwrap();
        assert_eq!(ra.spawn_bots(50).unwrap(), "Queued 50 bots");
        assert!(ra.spawn_bots(0).is_err() && ra.spawn_bots(5000).is_err());
        drop(ra);
        assert_eq!(h.join().unwrap()[2], "botcmd spawnleveled 50");
    }

    #[test]
    fn duplicate_account_and_input_validation() {
        let _lock = serial();
        let (port, _h) = fake_ra("Account already exist.");
        let mut ra = Ra::connect_to(port, "u", "p").unwrap();
        assert!(ra.create_account("Player1", "hunter22").unwrap_err().to_string().contains("already taken"));
        for (n, p) in [("ab", "hunter22"), ("bad name", "hunter22"), ("Player1", "short"), ("Player1", "has space1"), ("Player1", "quote\"pw1"), ("Pl\r\nayer", "hunter22"), ("x".repeat(18).as_str(), "hunter22")] {
            assert!(validate_account(n, p).is_err(), "{n:?} {p:?}");
        }
        validate_account("Player1", "hunter22").unwrap();
    }

    #[test]
    fn unreachable_console_gives_a_friendly_error() {
        let _lock = serial();
        // Port 1 is never used by a game server; probing a freed ephemeral port would race with the other tests.
        assert!(Ra::connect_to(1, "u", "p").unwrap_err().to_string().contains("not running"));
    }
}
