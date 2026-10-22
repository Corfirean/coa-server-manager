//! Windows Firewall rules for the game ports, owned by the Manager: fixed names, added only when missing (no duplicates),
//! removable by exact name (other rules are never touched). Reading needs no rights; changing needs one UAC confirmation.

use std::process::Command;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::layout::Ports;

pub const RULE_AUTH: &str = "CoA Server Manager - Auth";
pub const RULE_WORLD: &str = "CoA Server Manager - World";

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub auth: bool,
    pub world: bool,
}

fn hidden(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// `netsh ... show rule name=X` exits with 0 when at least one rule has exactly that name (locale independent).
pub fn rule_exists(name: &str) -> bool {
    let mut c = Command::new("netsh");
    c.args(["advfirewall", "firewall", "show", "rule"])
        .arg(format!("name={name}"));
    hidden(&mut c);
    c.output().map(|o| o.status.success()).unwrap_or(false)
}

pub fn status() -> Status {
    Status {
        auth: rule_exists(RULE_AUTH),
        world: rule_exists(RULE_WORLD),
    }
}

fn add_cmd(name: &str, port: u16) -> String {
    format!("netsh advfirewall firewall add rule name=\"{name}\" dir=in action=allow protocol=TCP localport={port} profile=private,domain")
}

fn delete_cmd(name: &str) -> String {
    format!("netsh advfirewall firewall delete rule name=\"{name}\"")
}

/// Commands that would make the rules match `ports`, given what exists now.
pub fn plan(ports: &Ports, have: &Status) -> Vec<String> {
    let mut v = Vec::new();
    if !have.auth {
        v.push(add_cmd(RULE_AUTH, ports.auth));
    }
    if !have.world {
        v.push(add_cmd(RULE_WORLD, ports.world));
    }
    v
}

/// Run `commands` with administrator rights (one UAC prompt). Fails cleanly if the user declines.
fn run_elevated(commands: &[String]) -> Result<()> {
    let script = std::env::temp_dir().join(format!("coa-firewall-{}.cmd", std::process::id()));
    std::fs::write(
        &script,
        format!("@echo off\r\n{}\r\n", commands.join("\r\n")),
    )?;
    let ps = format!("try {{ $p = Start-Process -FilePath cmd.exe -ArgumentList '/c','\"{}\"' -Verb RunAs -Wait -PassThru -WindowStyle Hidden; exit $p.ExitCode }} catch {{ exit 1223 }}", script.display());
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", &ps]);
    hidden(&mut c);
    let out = c.output();
    let _ = std::fs::remove_file(&script);
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) if o.status.code() == Some(1223) => Err(Error::Invalid(
            "Windows asked for permission and it was not given, so the firewall was not changed."
                .into(),
        )),
        _ => Err(Error::Invalid("The firewall could not be changed.".into())),
    }
}

/// Make sure both rules exist (adds only the missing ones) and confirm afterwards.
pub fn ensure_rules(ports: &Ports) -> Result<Status> {
    ensure_rules_with_secondary(ports, None)
}

pub fn ensure_rules_with_secondary(ports: &Ports, secondary: Option<u16>) -> Result<Status> {
    let have = status();
    let mut cmds = plan(ports, &have);
    let second_name = secondary.map(|port| format!("CoA Server Manager - Second World {port}"));
    if let (Some(port), Some(name)) = (secondary, &second_name) {
        if !rule_exists(name) {
            cmds.push(add_cmd(name, port));
        }
    }
    if !cmds.is_empty() {
        run_elevated(&cmds)?;
    }
    let now = status();
    if now.auth && now.world && second_name.as_ref().is_none_or(|name| rule_exists(name)) {
        Ok(now)
    } else {
        Err(Error::Invalid(
            "The firewall rules could not be confirmed after the change.".into(),
        ))
    }
}

/// Remove only the Manager's own rules.
pub fn remove_rules() -> Result<()> {
    let have = status();
    let mut cmds = Vec::new();
    if have.auth {
        cmds.push(delete_cmd(RULE_AUTH));
    }
    if have.world {
        cmds.push(delete_cmd(RULE_WORLD));
    }
    if cmds.is_empty() {
        return Ok(());
    }
    run_elevated(&cmds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ports() -> Ports {
        Ports {
            mysql: 3307,
            auth: 3724,
            world: 8085,
            ra: 3443,
        }
    }

    #[test]
    fn only_missing_rules_are_planned_and_only_game_ports_are_opened() {
        let all = plan(
            &ports(),
            &Status {
                auth: false,
                world: false,
            },
        );
        assert_eq!(all.len(), 2);
        assert!(all[0].contains("localport=3724") && all[1].contains("localport=8085"));
        assert!(
            !all.iter().any(|c| c.contains("3307") || c.contains("3443")),
            "database and console ports are never opened"
        );
        assert!(all.iter().all(|c| c.contains("profile=private,domain")
            && c.contains("action=allow")
            && c.contains("dir=in")));
        assert_eq!(
            plan(
                &ports(),
                &Status {
                    auth: true,
                    world: false
                }
            )
            .len(),
            1
        );
        assert!(
            plan(
                &ports(),
                &Status {
                    auth: true,
                    world: true
                }
            )
            .is_empty(),
            "no duplicates"
        );
        assert_eq!(
            delete_cmd(RULE_AUTH),
            "netsh advfirewall firewall delete rule name=\"CoA Server Manager - Auth\""
        );
    }

    #[test]
    fn reading_the_rules_needs_no_rights_and_finds_nothing_that_does_not_exist() {
        assert!(!rule_exists("CoA Server Manager - definitely not a rule"));
    }
}
