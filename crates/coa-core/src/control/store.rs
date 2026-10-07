//! The control plane's own SQLite database (`control.sqlite`). It holds **no secret**: no password, no key, no payload.
//!
//! * Host side: which PlayerId owns which game account of which local realm (and the public key the PlayerId first presented), and which realm
//!   characters were claimed by which PlayerId (a binding of a PortableCharacterId to a PlayerIdentity).
//! * Player side: for each remote realm the account name the player plays with (the password is in the secret store) and which characters came from it.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::{Error, Result};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS host_player(
    realm_local_id TEXT NOT NULL,
    player_id      TEXT NOT NULL,
    public_key     TEXT NOT NULL,
    account_id     INTEGER NOT NULL,
    username       TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('generated', 'linked')),
    created_at     TEXT NOT NULL,
    PRIMARY KEY (realm_local_id, player_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS host_player_account ON host_player(realm_local_id, account_id);
CREATE TABLE IF NOT EXISTS host_claim(
    realm_local_id TEXT NOT NULL,
    local_guid     INTEGER NOT NULL,
    character_id   TEXT NOT NULL,
    player_id      TEXT NOT NULL,
    state          TEXT NOT NULL CHECK (state IN ('exported', 'acknowledged')),
    claimed_at     TEXT NOT NULL,
    PRIMARY KEY (realm_local_id, local_guid)
);
CREATE INDEX IF NOT EXISTS host_claim_player ON host_claim(realm_local_id, player_id);
CREATE TABLE IF NOT EXISTS player_realm(
    realm_id   TEXT PRIMARY KEY,
    username   TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('generated', 'linked')),
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS realm_pin(
    realm_id   TEXT PRIMARY KEY,
    public_key TEXT NOT NULL,
    first_seen TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS player_claim(
    realm_id     TEXT NOT NULL,
    character_id TEXT NOT NULL,
    name         TEXT NOT NULL,
    claimed_at   TEXT NOT NULL,
    PRIMARY KEY (realm_id, character_id)
);
";

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn db(e: rusqlite::Error) -> Error {
    Error::Invalid(format!("the control database failed: {e}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountKind {
    Generated,
    Linked,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Generated => "generated",
            AccountKind::Linked => "linked",
        }
    }
    fn parse(s: &str) -> Self {
        if s == "linked" {
            AccountKind::Linked
        } else {
            AccountKind::Generated
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPlayer {
    pub player_id: Uuid,
    pub public_key: String,
    pub account_id: u32,
    pub username: String,
    pub kind: AccountKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimState {
    Exported,
    Acknowledged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostClaim {
    pub local_guid: u32,
    pub character_id: Uuid,
    pub player_id: Uuid,
    pub state: ClaimState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRealm {
    pub realm_id: String,
    pub username: String,
    pub kind: AccountKind,
}

pub struct ControlStore {
    conn: Connection,
}

fn uuid(s: String) -> Result<Uuid> {
    Uuid::parse_str(&s).map_err(|_| Error::Invalid("the control database holds a bad id".into()))
}

impl ControlStore {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join("control.sqlite")).map_err(db)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory().map_err(db)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(db)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(SCHEMA).map_err(db)?;
        Ok(Self { conn })
    }

    // ---- Host --------------------------------------------------------------------------------------------------------------------

    pub fn host_player(&self, realm: &str, player: &Uuid) -> Result<Option<HostPlayer>> {
        self.conn
            .query_row("SELECT player_id, public_key, account_id, username, kind FROM host_player WHERE realm_local_id = ?1 AND player_id = ?2", params![realm, player.to_string()], row_player)
            .optional()
            .map_err(db)?
            .transpose()
    }

    pub fn host_player_of_account(&self, realm: &str, account_id: u32) -> Result<Option<HostPlayer>> {
        self.conn
            .query_row("SELECT player_id, public_key, account_id, username, kind FROM host_player WHERE realm_local_id = ?1 AND account_id = ?2", params![realm, account_id], row_player)
            .optional()
            .map_err(db)?
            .transpose()
    }

    pub fn host_bind_player(&self, realm: &str, player: &Uuid, public_key: &str, account_id: u32, username: &str, kind: AccountKind) -> Result<()> {
        self.conn
            .execute("INSERT INTO host_player(realm_local_id, player_id, public_key, account_id, username, kind, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)", params![realm, player.to_string(), public_key, account_id, username, kind.as_str(), now()])
            .map_err(db)?;
        Ok(())
    }

    pub fn host_forget_player(&self, realm: &str, player: &Uuid) -> Result<()> {
        self.conn.execute("DELETE FROM host_player WHERE realm_local_id = ?1 AND player_id = ?2", params![realm, player.to_string()]).map_err(db)?;
        Ok(())
    }

    pub fn host_claim(&self, realm: &str, local_guid: u32) -> Result<Option<HostClaim>> {
        self.conn
            .query_row("SELECT local_guid, character_id, player_id, state FROM host_claim WHERE realm_local_id = ?1 AND local_guid = ?2", params![realm, local_guid], row_claim)
            .optional()
            .map_err(db)?
            .transpose()
    }

    pub fn host_claims_of(&self, realm: &str, player: &Uuid) -> Result<Vec<HostClaim>> {
        let mut stmt = self.conn.prepare("SELECT local_guid, character_id, player_id, state FROM host_claim WHERE realm_local_id = ?1 AND player_id = ?2 ORDER BY local_guid").map_err(db)?;
        let rows = stmt.query_map(params![realm, player.to_string()], row_claim).map_err(db)?;
        rows.map(|r| r.map_err(db)?).collect()
    }

    pub fn host_claim_set(&self, realm: &str, local_guid: u32, character: &Uuid, player: &Uuid) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO host_claim(realm_local_id, local_guid, character_id, player_id, state, claimed_at) VALUES (?1, ?2, ?3, ?4, 'exported', ?5)
                 ON CONFLICT(realm_local_id, local_guid) DO UPDATE SET character_id = excluded.character_id WHERE host_claim.player_id = excluded.player_id",
                params![realm, local_guid, character.to_string(), player.to_string(), now()],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn host_claim_acknowledge(&self, realm: &str, character: &Uuid, player: &Uuid) -> Result<bool> {
        let n = self.conn.execute("UPDATE host_claim SET state = 'acknowledged' WHERE realm_local_id = ?1 AND character_id = ?2 AND player_id = ?3", params![realm, character.to_string(), player.to_string()]).map_err(db)?;
        Ok(n == 1)
    }

    // ---- Player ------------------------------------------------------------------------------------------------------------------

    pub fn player_realm(&self, realm_id: &str) -> Result<Option<PlayerRealm>> {
        self.conn
            .query_row("SELECT realm_id, username, kind FROM player_realm WHERE realm_id = ?1", [realm_id], |r| Ok(PlayerRealm { realm_id: r.get(0)?, username: r.get(1)?, kind: AccountKind::parse(&r.get::<_, String>(2)?) }))
            .optional()
            .map_err(db)
    }

    pub fn player_realm_set(&self, realm_id: &str, username: &str, kind: AccountKind) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO player_realm(realm_id, username, kind, created_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(realm_id) DO UPDATE SET username = excluded.username, kind = excluded.kind",
                params![realm_id, username, kind.as_str(), now()],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn player_realm_remove(&self, realm_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM player_realm WHERE realm_id = ?1", [realm_id]).map_err(db)?;
        Ok(())
    }

    /// The public key a realm had when this player first reached it. A realm id never changes its key (a new key is a new realm), so a different one is refused.
    pub fn realm_pin(&self, realm_id: &str) -> Result<Option<String>> {
        self.conn.query_row("SELECT public_key FROM realm_pin WHERE realm_id = ?1", [realm_id], |r| r.get(0)).optional().map_err(db)
    }

    pub fn realm_pin_set(&self, realm_id: &str, public_key: &str) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO realm_pin(realm_id, public_key, first_seen) VALUES (?1, ?2, ?3)", params![realm_id, public_key, now()]).map_err(db)?;
        Ok(())
    }

    pub fn player_claim_add(&self, realm_id: &str, character: &Uuid, name: &str) -> Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO player_claim(realm_id, character_id, name, claimed_at) VALUES (?1, ?2, ?3, ?4)", params![realm_id, character.to_string(), name, now()]).map_err(db)?;
        Ok(())
    }

    pub fn player_claims(&self, realm_id: &str) -> Result<Vec<(Uuid, String)>> {
        let mut stmt = self.conn.prepare("SELECT character_id, name FROM player_claim WHERE realm_id = ?1 ORDER BY claimed_at").map_err(db)?;
        let rows = stmt.query_map([realm_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(db)?;
        rows.map(|r| {
            let (id, name) = r.map_err(db)?;
            Ok((uuid(id)?, name))
        })
        .collect()
    }
}

fn row_player(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<HostPlayer>> {
    let (id, key, account, username, kind): (String, String, u32, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?);
    Ok(uuid(id).map(|player_id| HostPlayer { player_id, public_key: key, account_id: account, username, kind: AccountKind::parse(&kind) }))
}

fn row_claim(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<HostClaim>> {
    let (guid, cid, pid, state): (u32, String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
    Ok((|| Ok(HostClaim { local_guid: guid, character_id: uuid(cid)?, player_id: uuid(pid)?, state: if state == "acknowledged" { ClaimState::Acknowledged } else { ClaimState::Exported } }))())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_owns_one_account_per_realm_and_an_account_one_player() {
        let s = ControlStore::open_in_memory().unwrap();
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        s.host_bind_player("srv-1", &a, "KEY-A", 7, "DMITRY", AccountKind::Generated).unwrap();
        assert!(s.host_bind_player("srv-1", &a, "KEY-A", 8, "OTHER", AccountKind::Generated).is_err(), "one account per player and realm");
        assert!(s.host_bind_player("srv-1", &b, "KEY-B", 7, "DMITRY", AccountKind::Linked).is_err(), "one player per account");
        s.host_bind_player("srv-2", &a, "KEY-A", 7, "DMITRY", AccountKind::Generated).unwrap();
        let p = s.host_player("srv-1", &a).unwrap().unwrap();
        assert_eq!((p.account_id, p.username.as_str(), p.public_key.as_str(), p.kind), (7, "DMITRY", "KEY-A", AccountKind::Generated));
        assert_eq!(s.host_player_of_account("srv-1", 7).unwrap().unwrap().player_id, a);
        assert!(s.host_player("srv-1", &b).unwrap().is_none());
        s.host_forget_player("srv-1", &a).unwrap();
        assert!(s.host_player("srv-1", &a).unwrap().is_none() && s.host_player("srv-2", &a).unwrap().is_some());
    }

    #[test]
    fn a_character_is_claimed_by_one_player_and_acknowledged_once() {
        let s = ControlStore::open_in_memory().unwrap();
        let (p, q, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        s.host_claim_set("srv-1", 50, &c, &p).unwrap();
        assert_eq!(s.host_claim("srv-1", 50).unwrap().unwrap().state, ClaimState::Exported);
        s.host_claim_set("srv-1", 50, &c, &p).unwrap();
        s.host_claim_set("srv-1", 50, &Uuid::now_v7(), &q).unwrap();
        let held = s.host_claim("srv-1", 50).unwrap().unwrap();
        assert_eq!((held.player_id, held.character_id), (p, c), "another player cannot take over a claim");
        assert!(!s.host_claim_acknowledge("srv-1", &c, &q).unwrap(), "only the claiming player acknowledges");
        assert!(s.host_claim_acknowledge("srv-1", &c, &p).unwrap());
        assert_eq!(s.host_claim("srv-1", 50).unwrap().unwrap().state, ClaimState::Acknowledged);
        assert_eq!(s.host_claims_of("srv-1", &p).unwrap().len(), 1);
        assert!(s.host_claims_of("srv-1", &q).unwrap().is_empty());
    }

    #[test]
    fn the_player_remembers_account_names_only() {
        let dir = tempfile::tempdir().unwrap();
        {
            let s = ControlStore::open(dir.path()).unwrap();
            s.player_realm_set("realm-1", "DMITRY_7K4M", AccountKind::Generated).unwrap();
            s.player_claim_add("realm-1", &Uuid::now_v7(), "Thrall").unwrap();
        }
        let s = ControlStore::open(dir.path()).unwrap();
        assert_eq!(s.player_realm("realm-1").unwrap().unwrap().username, "DMITRY_7K4M");
        assert_eq!(s.player_claims("realm-1").unwrap().len(), 1);
        s.player_realm_remove("realm-1").unwrap();
        assert!(s.player_realm("realm-1").unwrap().is_none());
        let raw = std::fs::read(dir.path().join("control.sqlite")).unwrap();
        assert!(!String::from_utf8_lossy(&raw).to_lowercase().contains("password"), "the schema has no password column");
    }
}
