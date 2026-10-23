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
        .and_then(|c| {
            c.get("CoaBots.RandomSpawn.AccountPrefix")
                .map(|v| unquote(v).to_string())
        })
        .filter(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "CoaBotHost".into())
}

fn parse_counts(out: &str) -> Result<Population> {
    let f: Vec<u32> = out
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    if f.len() != 3 {
        return Err(Error::Invalid(format!(
            "unexpected population reply: {out:?}"
        )));
    }
    let (online_total, bots_online, bots_total) = (f[0], f[1], f[2]);
    Ok(Population {
        online_total,
        bots_online,
        players_online: online_total.saturating_sub(bots_online),
        bots_total,
    })
}

/// Read-only query with the game's own database account. Requires the database to be running.
pub fn query(root: &Path) -> Result<Population> {
    let db = Db::from_repack(root, Account::App)?;
    query_with(
        &db,
        &bot_account_prefix(root),
        &crate::accounts::squid_bot_prefix(root),
    )
}

fn counts_sql(prefix: &str, squid_prefix: &str) -> Result<String> {
    if [prefix, squid_prefix]
        .iter()
        .any(|prefix| prefix.is_empty() || !prefix.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return Err(Error::Invalid(
            "the bot account prefix is not a plain name".into(),
        ));
    }
    let prefix = prefix.to_ascii_uppercase();
    let squid_prefix = squid_prefix.to_ascii_uppercase();
    // OR counts each character once even when the two configured prefixes overlap.
    let bots =
        format!("(UPPER(a.username) LIKE '{prefix}%' OR UPPER(a.username) LIKE '{squid_prefix}%')");
    Ok(format!(
        "SELECT (SELECT COUNT(*) FROM acore_characters.characters WHERE online=1), \
                (SELECT COUNT(*) FROM acore_characters.characters c JOIN acore_auth.account a ON a.id=c.account WHERE c.online=1 AND {bots}), \
                (SELECT COUNT(*) FROM acore_characters.characters c JOIN acore_auth.account a ON a.id=c.account WHERE {bots});"
    ))
}

/// Read both supported bot populations, including SQUID random bots, without changing realm data.
pub fn query_with(db: &Db, prefix: &str, squid_prefix: &str) -> Result<Population> {
    parse_counts(&db.query(&counts_sql(prefix, squid_prefix)?)?)
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
        cores: std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(2),
        ram_gb: if ok { gb(st.ullTotalPhys) } else { 8.0 },
        free_ram_gb: if ok { gb(st.ullAvailPhys) } else { 4.0 },
    }
}

#[cfg(not(windows))]
pub fn hardware() -> Hardware {
    let (total, free) = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|t| parse_meminfo(&t))
        .unwrap_or((8.0, 4.0));
    Hardware {
        cores: std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(2),
        ram_gb: total,
        free_ram_gb: free,
    }
}

/// (total, available) memory in GB from the text of `/proc/meminfo`.
#[cfg(not(windows))]
fn parse_meminfo(text: &str) -> Option<(f64, f64)> {
    let kb = |key: &str| -> Option<f64> {
        text.lines().find_map(|l| {
            l.strip_prefix(key)?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse::<f64>()
                .ok()
        })
    };
    Some((
        kb("MemTotal:")? / (1u64 << 20) as f64,
        kb("MemAvailable:")? / (1u64 << 20) as f64,
    ))
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
        SizeOption {
            id: "small",
            title: "Small",
            bots: 50,
            warning: None,
        },
        SizeOption {
            id: "medium",
            title: "Medium",
            bots: 250,
            warning: need(250, 4, 8.0),
        },
        SizeOption {
            id: "large",
            title: "Large",
            bots: 500,
            warning: need(500, 8, 16.0),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Execute the production SELECT against disposable in-memory schemas. No
    // server connection or credentials are needed for these regression cases.
    fn fixture_counts(
        prefix: &str,
        squid_prefix: &str,
        accounts: &[(&str, u32, u32)],
    ) -> Population {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(
            "ATTACH DATABASE ':memory:' AS acore_auth;
            ATTACH DATABASE ':memory:' AS acore_characters;
            CREATE TABLE acore_auth.account (id INTEGER PRIMARY KEY, username TEXT);
            CREATE TABLE acore_characters.characters (account INTEGER, online INTEGER);",
        )
        .unwrap();
        for (index, (name, online, offline)) in accounts.iter().enumerate() {
            db.execute(
                "INSERT INTO acore_auth.account VALUES (?1, ?2)",
                rusqlite::params![index as u32, name],
            )
            .unwrap();
            for flag in std::iter::repeat_n(1, *online as usize)
                .chain(std::iter::repeat_n(0, *offline as usize))
            {
                db.execute(
                    "INSERT INTO acore_characters.characters VALUES (?1, ?2)",
                    rusqlite::params![index as u32, flag],
                )
                .unwrap();
            }
        }
        let changes = db.total_changes();
        let (total, bots, all): (u32, u32, u32) = db
            .query_row(&counts_sql(prefix, squid_prefix).unwrap(), [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(
            db.total_changes(),
            changes,
            "population queries must only read"
        );
        parse_counts(&format!("{total}\t{bots}\t{all}")).unwrap()
    }

    #[test]
    fn random_bots_are_not_advertised_as_real_players() {
        let p = fixture_counts(
            "CoaBotHost",
            "rndbot",
            &[("RNDBOT1", 171, 1), ("CoaBotHost1", 2, 3), ("PLAYER", 2, 1)],
        );
        assert_eq!(
            (
                p.online_total,
                p.players_online,
                p.bots_online,
                p.bots_total
            ),
            (175, 2, 173, 177)
        );
        let empty = fixture_counts("CoaBotHost", "rndbot", &[]);
        assert_eq!(
            (empty.online_total, empty.bots_online, empty.bots_total),
            (0, 0, 0)
        );
        let humans = fixture_counts("CoaBotHost", "rndbot", &[("PLAYER", 3, 1)]);
        assert_eq!((humans.players_online, humans.bots_online), (3, 0));
    }

    #[test]
    fn population_uses_both_configured_prefixes_and_counts_overlaps_once() {
        let root = tempfile::tempdir().unwrap();
        let configs = root.path().join("Core/configs/modules");
        std::fs::create_dir_all(&configs).unwrap();
        std::fs::write(
            configs.join("mod_coa_playerbots.conf"),
            "CoaBots.RandomSpawn.AccountPrefix = \"Companion\"\n",
        )
        .unwrap();
        std::fs::write(
            configs.join("playerbots.conf"),
            "AiPlayerbot.RandomBotAccountPrefix = \"Random\"\n",
        )
        .unwrap();
        let p = fixture_counts(
            &bot_account_prefix(root.path()),
            &crate::accounts::squid_bot_prefix(root.path()),
            &[("COMPANION1", 2, 0), ("RANDOM1", 3, 1), ("PLAYER", 1, 0)],
        );
        assert_eq!((p.players_online, p.bots_online, p.bots_total), (1, 5, 6));
        let p = fixture_counts(
            "Bot",
            "BotRandom",
            &[("BOTRANDOM1", 3, 1), ("BOT1", 2, 0), ("PLAYER", 1, 0)],
        );
        assert_eq!((p.players_online, p.bots_online, p.bots_total), (1, 5, 6));
        for unsafe_prefix in ["", "x' OR 1=1 --", "bot_", "bot%"] {
            assert!(counts_sql(unsafe_prefix, "rndbot").is_err());
            assert!(counts_sql("CoaBotHost", unsafe_prefix).is_err());
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn memory_is_read_from_proc_meminfo() {
        let t = "MemTotal:       65536000 kB\nMemFree:         1000000 kB\nMemAvailable:   16777216 kB\nBuffers: 1 kB\n";
        let (total, free) = parse_meminfo(t).unwrap();
        assert!(
            (total - 62.5).abs() < 0.01 && (free - 16.0).abs() < 0.01,
            "{total} {free}"
        );
        assert!(parse_meminfo("nothing useful").is_none());
        assert!(hardware().ram_gb > 0.0);
    }

    #[test]
    fn parses_counts_and_never_goes_negative() {
        let p = parse_counts("12\t9\t400").unwrap();
        assert_eq!(
            (
                p.online_total,
                p.bots_online,
                p.players_online,
                p.bots_total
            ),
            (12, 9, 3, 400)
        );
        assert_eq!(parse_counts("5\t7\t7").unwrap().players_online, 0);
        assert!(parse_counts("garbage").is_err());
    }

    #[test]
    fn prefix_comes_from_the_bot_config_and_is_sanitised() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(bot_account_prefix(d.path()), "CoaBotHost");
        let f = d.path().join("Core/configs/modules");
        std::fs::create_dir_all(&f).unwrap();
        std::fs::write(
            f.join("mod_coa_playerbots.conf"),
            "CoaBots.RandomSpawn.AccountPrefix = \"MyBots\"\n",
        )
        .unwrap();
        assert_eq!(bot_account_prefix(d.path()), "MyBots");
        std::fs::write(
            f.join("mod_coa_playerbots.conf"),
            "CoaBots.RandomSpawn.AccountPrefix = \"x' OR 1=1 --\"\n",
        )
        .unwrap();
        assert_eq!(
            bot_account_prefix(d.path()),
            "CoaBotHost",
            "anything that is not letters/digits is ignored (SQL safety)"
        );
    }

    #[test]
    fn sizes_warn_on_weak_hardware_only() {
        let weak = sizes(&Hardware {
            cores: 2,
            ram_gb: 4.0,
            free_ram_gb: 2.0,
        });
        assert!(
            weak[0].warning.is_none() && weak[1].warning.is_some() && weak[2].warning.is_some()
        );
        let strong = sizes(&Hardware {
            cores: 16,
            ram_gb: 32.0,
            free_ram_gb: 20.0,
        });
        assert!(strong.iter().all(|s| s.warning.is_none()));
        let busy = sizes(&Hardware {
            cores: 16,
            ram_gb: 32.0,
            free_ram_gb: 2.0,
        });
        assert!(busy[2].warning.as_deref().unwrap().contains("memory"));
    }
}
