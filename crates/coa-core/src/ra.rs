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
            got
        });
        (port, h)
    }

    #[test]
    fn creates_an_account_over_ra_with_the_expected_commands() {
        let (port, h) = fake_ra("Account created: PLAYER1");
        let mut ra = Ra::connect_to(port, "local", "secretra").unwrap();
        ra.create_account("Player1", "hunter22").unwrap();
        drop(ra);
        assert_eq!(h.join().unwrap(), ["local", "secretra", "account create Player1 hunter22"]);
    }

    #[test]
    fn duplicate_account_and_input_validation() {
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
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        assert!(Ra::connect_to(port, "u", "p").unwrap_err().to_string().contains("not running"));
    }
}
