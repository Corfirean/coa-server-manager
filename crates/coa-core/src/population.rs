//! Who is online (real players vs bots) and what the machine can carry.

use std::path::Path;

use serde::Serialize;

use crate::config::parser::{unquote, ConfFile};
use crate::db::{Account, Db};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct Population {
    pub online_total: u32,
    pub bots_online: u32,
    pub players_online: u32,
    /// Bot characters that exist (online or not).
    pub bots_total: u32,
}

/// Bot accounts share a name prefix (`CoaBots.RandomSpawn.AccountPrefix`, default `CoaBotHost`).
pub fn bot_account_prefix(root: &Path) -> String {
    std::fs::read(root.join("Core/configs/modules/mod_coa_playerbots.conf"))
        .ok()
        .and_then(|b| ConfFile::parse_bytes(&b).ok())
        .and_then(|c| c.get("CoaBots.RandomSpawn.AccountPrefix").map(|v| unquote(v).to_string()))
        .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "CoaBotHost".into())
}

fn parse_counts(out: &str) -> Result<Population> {
    let f: Vec<u32> = out.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if f.len() != 3 {
        return Err(Error::Invalid(format!("unexpected population reply: {out:?}")));
    }
    let (online_total, bots_online, bots_total) = (f[0], f[1], f[2]);
    Ok(Population { online_total, bots_online, players_online: online_total.saturating_sub(bots_online), bots_total })
}

/// Read-only query with the game's own database account. Requires the database to be running.
pub fn query(root: &Path) -> Result<Population> {
    let prefix = bot_account_prefix(root).to_uppercase();
    let db = Db::from_repack(root, Account::App)?;
    let sql = format!(
        "SELECT (SELECT COUNT(*) FROM acore_characters.characters WHERE online=1), \
                (SELECT COUNT(*) FROM acore_characters.characters c JOIN acore_auth.account a ON a.id=c.account WHERE c.online=1 AND UPPER(a.username) LIKE '{prefix}%'), \
                (SELECT COUNT(*) FROM acore_characters.characters c JOIN acore_auth.account a ON a.id=c.account WHERE UPPER(a.username) LIKE '{prefix}%');"
    );
    parse_counts(&db.query(&sql)?)
}

#[derive(Debug, Clone, Serialize)]
pub struct Hardware {
    pub cores: u32,
    pub ram_gb: f64,
    pub free_ram_gb: f64,
}

#[cfg(windows)]
pub fn hardware() -> Hardware {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    st.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    let ok = unsafe { GlobalMemoryStatusEx(&mut st) } != 0;
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    Hardware {
        cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2),
        ram_gb: if ok { gb(st.ullTotalPhys) } else { 8.0 },
        free_ram_gb: if ok { gb(st.ullAvailPhys) } else { 4.0 },
    }
}

#[cfg(not(windows))]
pub fn hardware() -> Hardware {
    let (total, free) = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| parse_meminfo(&t)).unwrap_or((8.0, 4.0));
    Hardware { cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(2), ram_gb: total, free_ram_gb: free }
}

/// (total, available) memory in GB from the text of `/proc/meminfo`.
#[cfg(not(windows))]
fn parse_meminfo(text: &str) -> Option<(f64, f64)> {
    let kb = |key: &str| -> Option<f64> { text.lines().find_map(|l| l.strip_prefix(key)?.trim().strip_suffix("kB")?.trim().parse::<f64>().ok()) };
    Some((kb("MemTotal:")? / (1u64 << 20) as f64, kb("MemAvailable:")? / (1u64 << 20) as f64))
}

#[derive(Debug, Clone, Serialize)]
pub struct SizeOption {
    pub id: &'static str,
    pub title: &'static str,
    pub bots: u32,
    /// None = comfortable on this machine; Some = why to think twice.
    pub warning: Option<String>,
}

/// Recommended sizes with a hardware-aware note. Nothing is forbidden: advanced users may exceed them.
pub fn sizes(h: &Hardware) -> Vec<SizeOption> {
    let need = |bots: u32, cores: u32, ram: f64| -> Option<String> {
        if h.cores < cores || h.ram_gb < ram {
            Some(format!("{bots} companions may need more CPU and memory than this computer has ({} cores, {:.0} GB).", h.cores, h.ram_gb))
        } else if h.free_ram_gb < ram / 2.0 {
            Some("Free some memory first; other programs are using most of it.".into())
        } else {
            None
        }
    };
    vec![
        SizeOption { id: "small", title: "Small", bots: 50, warning: None },
        SizeOption { id: "medium", title: "Medium", bots: 250, warning: need(250, 4, 8.0) },
        SizeOption { id: "large", title: "Large", bots: 500, warning: need(500, 8, 16.0) },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn memory_is_read_from_proc_meminfo() {
        let t = "MemTotal:       65536000 kB\nMemFree:         1000000 kB\nMemAvailable:   16777216 kB\nBuffers: 1 kB\n";
        let (total, free) = parse_meminfo(t).unwrap();
        assert!((total - 62.5).abs() < 0.01 && (free - 16.0).abs() < 0.01, "{total} {free}");
        assert!(parse_meminfo("nothing useful").is_none());
        assert!(hardware().ram_gb > 0.0);
    }

    #[test]
    fn parses_counts_and_never_goes_negative() {
        let p = parse_counts("12\t9\t400").unwrap();
        assert_eq!((p.online_total, p.bots_online, p.players_online, p.bots_total), (12, 9, 3, 400));
        assert_eq!(parse_counts("5\t7\t7").unwrap().players_online, 0);
        assert!(parse_counts("garbage").is_err());
    }

    #[test]
    fn prefix_comes_from_the_bot_config_and_is_sanitised() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(bot_account_prefix(d.path()), "CoaBotHost");
        let f = d.path().join("Core/configs/modules");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(f.join("mod_coa_playerbots.conf"), "CoaBots.RandomSpawn.AccountPrefix = \"MyBots\"\n").unwrap();
        assert_eq!(bot_account_prefix(d.path()), "MyBots");
        std::fs::write(f.join("mod_coa_playerbots.conf"), "CoaBots.RandomSpawn.AccountPrefix = \"x' OR 1=1 --\"\n").unwrap();
        assert_eq!(bot_account_prefix(d.path()), "CoaBotHost", "anything that is not letters/digits is ignored (SQL safety)");
    }

    #[test]
    fn sizes_warn_on_weak_hardware_only() {
        let weak = sizes(&Hardware { cores: 2, ram_gb: 4.0, free_ram_gb: 2.0 });
        assert!(weak[0].warning.is_none() && weak[1].warning.is_some() && weak[2].warning.is_some());
        let strong = sizes(&Hardware { cores: 16, ram_gb: 32.0, free_ram_gb: 20.0 });
        assert!(strong.iter().all(|s| s.warning.is_none()));
        let busy = sizes(&Hardware { cores: 16, ram_gb: 32.0, free_ram_gb: 2.0 });
        assert!(busy[2].warning.as_deref().unwrap().contains("memory"));
    }
}
