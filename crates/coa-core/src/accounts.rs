//! The accounts of the people who play on this server: listed from the login database, changed through the world
//! console (password, access level) or - for a rename - by changing the stored name and then letting the console set
//! a new password, because the login verifier is computed from the name and would no longer match.
//! The accounts the Manager and the companions use themselves are never listed.

use std::path::Path;

use serde::Serialize;

use crate::db::{Account, Db};
use crate::error::{Error, Result};
use crate::ra::{validate_account, Ra};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AccountInfo {
    pub id: u32,
    pub name: String,
    /// 0 player, 1 moderator, 2 game master, 3 administrator.
    pub access: u8,
    pub online: bool,
    pub last_login: Option<String>,
    pub characters: u32,
}

const LIST_SQL: &str = "SELECT a.id, a.username, \
    IFNULL((SELECT MAX(aa.gmlevel) FROM acore_auth.account_access aa WHERE aa.id = a.id), 0), \
    a.online, IFNULL(a.last_login, ''), \
    (SELECT COUNT(*) FROM acore_characters.characters c WHERE c.account = a.id) \
    FROM acore_auth.account a \
    WHERE a.username NOT LIKE 'COABOTHOST%' AND a.username <> 'COAMANAGER' AND a.username NOT LIKE '{SQUID}%' ORDER BY a.id;";

/// The accounts of SQUID's random bots share a name prefix (`AiPlayerbot.RandomBotAccountPrefix`, default `rndbot`).
/// They belong to the bot system like the companions' accounts and are never listed or touched here.
pub fn squid_bot_prefix(root: &Path) -> String {
    std::fs::read(root.join("Core/configs/modules/playerbots.conf"))
        .ok()
        .and_then(|bytes| crate::config::parser::ConfFile::parse_bytes(&bytes).ok())
        .and_then(|conf| conf.get("AiPlayerbot.RandomBotAccountPrefix").map(|v| crate::config::parser::unquote(v).to_string()))
        .filter(|prefix| !prefix.is_empty() && prefix.len() <= 32 && prefix.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "rndbot".into())
}

/// Rows of tab-separated columns as the `mysql` client prints them.
pub fn parse_list(out: &str) -> Vec<AccountInfo> {
    out.lines()
        .filter_map(|line| {
            let c: Vec<&str> = line.split('\t').collect();
            if c.len() < 6 {
                return None;
            }
            Some(AccountInfo {
                id: c[0].trim().parse().ok()?,
                name: c[1].trim().to_string(),
                access: c[2].trim().parse().unwrap_or(0),
                online: c[3].trim() != "0",
                last_login: Some(c[4].trim().to_string()).filter(|s| !s.is_empty() && s != "NULL"),
                characters: c[5].trim().parse().unwrap_or(0),
            })
        })
        .collect()
}

pub fn list(root: &Path) -> Result<Vec<AccountInfo>> {
    let db = Db::from_repack(root, Account::Admin)?;
    Ok(parse_list(&db.query(&LIST_SQL.replace("{SQUID}", &squid_bot_prefix(root).to_ascii_uppercase()))?))
}

fn upper(name: &str) -> Result<String> {
    validate_account(name, "placeholder")?;
    Ok(name.to_ascii_uppercase())
}

/// Give the account a new name. Needs the world console (the new password is what makes the login work again) and an
/// account that is not logged in. The old name is put back if anything goes wrong.
pub fn rename(root: &Path, ra: &mut Ra, old: &str, new: &str, password: &str) -> Result<()> {
    let (old, new) = (upper(old)?, upper(new)?);
    validate_account(&new, password)?;
    if is_reserved(&old) || old.starts_with(&squid_bot_prefix(root).to_ascii_uppercase()) {
        return Err(Error::Invalid("This account belongs to the Manager or the bots and cannot be renamed.".into()));
    }
    if old == new {
        return Err(Error::Invalid("That is already the name of this account.".into()));
    }
    let db = Db::from_repack(root, Account::Admin)?;
    let taken = db.query(&format!("SELECT COUNT(*) FROM acore_auth.account WHERE username = '{new}';"))?;
    if taken.trim() != "0" {
        return Err(Error::Invalid("That account name is already taken.".into()));
    }
    let changed = db.query(&format!("UPDATE acore_auth.account SET username = '{new}' WHERE username = '{old}' AND online = 0; SELECT ROW_COUNT();"))?;
    if changed.trim() != "1" {
        return Err(Error::Invalid("The account was not renamed: it does not exist or is logged in right now.".into()));
    }
    if let Err(e) = ra.set_account_password(&new, password) {
        let _ = db.query(&format!("UPDATE acore_auth.account SET username = '{old}' WHERE username = '{new}';"));
        return Err(e);
    }
    tracing::info!(%old, %new, "account renamed");
    Ok(())
}

/// Accounts the Manager and the companions use themselves; they can never be deleted from here.
fn is_reserved(upper_name: &str) -> bool {
    upper_name == "COAMANAGER" || upper_name.starts_with("COABOT")
}

/// Delete an account and, with it, all of its characters. Needs the world console. Refuses the Manager's and the
/// companions' own accounts, an account that is not there and one that is logged in right now.
pub fn delete(root: &Path, ra: &mut Ra, name: &str) -> Result<()> {
    let name = upper(name)?;
    if is_reserved(&name) || name.starts_with(&squid_bot_prefix(root).to_ascii_uppercase()) {
        return Err(Error::Invalid("This account belongs to the Manager or the bots and cannot be deleted.".into()));
    }
    let db = Db::from_repack(root, Account::Admin)?;
    match db.query(&format!("SELECT online FROM acore_auth.account WHERE username = '{name}';"))?.trim() {
        "" => return Err(Error::Invalid("That account does not exist.".into())),
        "0" => {}
        _ => return Err(Error::Invalid("The account is logged in right now; log it out first.".into())),
    }
    ra.delete_account(&name)?;
    if db.query(&format!("SELECT COUNT(*) FROM acore_auth.account WHERE username = '{name}';"))?.trim() != "0" {
        return Err(Error::Invalid("The account was not deleted.".into()));
    }
    tracing::info!(%name, "account deleted");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_from_the_client_become_accounts() {
        let out = "2\tALICE\t3\t0\t2026-09-30 21:04:11\t4\n5\tBOB\t0\t1\t\t0\n7\tBROKEN\n";
        let a = parse_list(out);
        assert_eq!(a.len(), 2, "a short row is skipped");
        assert_eq!(a[0], AccountInfo { id: 2, name: "ALICE".into(), access: 3, online: false, last_login: Some("2026-09-30 21:04:11".into()), characters: 4 });
        assert!(a[1].online && a[1].last_login.is_none() && a[1].access == 0);
    }

    #[test]
    fn the_internal_accounts_are_filtered_out_by_the_query() {
        assert!(LIST_SQL.contains("COABOTHOST%") && LIST_SQL.contains("COAMANAGER") && LIST_SQL.contains("{SQUID}%"));
    }

    #[test]
    fn the_squid_bot_prefix_comes_from_the_bot_settings_and_defaults_to_rndbot() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(squid_bot_prefix(root.path()), "rndbot");
        let conf = root.path().join("Core/configs/modules/playerbots.conf");
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "AiPlayerbot.RandomBotAccountPrefix = \"MyBots\"\n").unwrap();
        assert_eq!(squid_bot_prefix(root.path()), "MyBots");
        std::fs::write(&conf, "AiPlayerbot.RandomBotAccountPrefix = \"x' OR 1=1 --\"\n").unwrap();
        assert_eq!(squid_bot_prefix(root.path()), "rndbot", "an unsafe prefix is ignored");
    }

    #[test]
    fn the_managers_and_the_companions_accounts_are_never_deletable() {
        for n in ["COAMANAGER", "COABOTHOST1", "COABOT12", "COABOTS"] {
            assert!(is_reserved(n), "{n}");
        }
        for n in ["ALICE", "MYCOABOT", "BOT1"] {
            assert!(!is_reserved(n), "{n}");
        }
    }

    #[test]
    fn names_are_validated_and_stored_in_capitals() {
        assert_eq!(upper("Alice1").unwrap(), "ALICE1");
        assert!(upper("a").is_err() && upper("x; DROP TABLE account").is_err() && upper("bad name").is_err());
    }
}
