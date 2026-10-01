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
    WHERE a.username NOT LIKE 'COABOTHOST%' AND a.username <> 'COAMANAGER' ORDER BY a.id;";

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
    Ok(parse_list(&db.query(LIST_SQL)?))
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
        assert!(LIST_SQL.contains("COABOTHOST%") && LIST_SQL.contains("COAMANAGER"));
    }

    #[test]
    fn names_are_validated_and_stored_in_capitals() {
        assert_eq!(upper("Alice1").unwrap(), "ALICE1");
        assert!(upper("a").is_err() && upper("x; DROP TABLE account").is_err() && upper("bad name").is_err());
    }
}
