//! Minimal Remote Access (RA) console client for the worldserver, local only.
//! Only a fixed set of typed operations is exposed; free-form commands are not accepted here.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

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

    /// Ask the bot module to create `count` leveling bots (throttled by the module itself). Only a number is sent.
    pub fn spawn_bots(&mut self, count: u32) -> Result<String> {
        if !(1..=2000).contains(&count) {
            return Err(Error::Invalid("Choose between 1 and 2000 companions.".into()));
        }
        let out = self.command(&format!("botcmd spawnleveled {count}"))?;
        Ok(out.lines().last().unwrap_or("").to_string())
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
    fn creates_an_account_over_ra_with_the_expected_commands() {
        let _lock = serial();
        let (port, h) = fake_ra("Account created: PLAYER1");
        let mut ra = Ra::connect_to(port, "local", "secretra").unwrap();
        ra.create_account("Player1", "hunter22").unwrap();
        drop(ra);
        assert_eq!(h.join().unwrap(), ["local", "secretra", "account create Player1 hunter22"]);
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
