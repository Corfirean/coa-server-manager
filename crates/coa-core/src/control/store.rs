//! The control plane's own SQLite database (`control.sqlite`). It holds **no secret**: no password, no key, no payload.
//!
//! * Host side: which PlayerId owns which game account of which local realm (and the public key the PlayerId first presented), and which realm
//!   characters were claimed by which PlayerId (a binding of a PortableCharacterId to a PlayerIdentity).
//! * Player side: for each remote realm the account name the player plays with (the password is in the secret store) and which characters came from it.

use std::collections::BTreeMap;
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
CREATE TABLE IF NOT EXISTS host_transfer(
    realm_local_id  TEXT NOT NULL,
    transfer_id     TEXT NOT NULL PRIMARY KEY,
    character_id    TEXT NOT NULL,
    player_id       TEXT NOT NULL,
    revision        INTEGER NOT NULL,
    content_hash    TEXT NOT NULL,
    total_size      INTEGER NOT NULL,
    received_size   INTEGER NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('receiving', 'committed', 'acknowledged')),
    local_guid      INTEGER,
    session_id      TEXT,
    projected_level INTEGER,
    notes           TEXT,
    collections     TEXT NOT NULL DEFAULT '{}',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS host_transfer_char ON host_transfer(realm_local_id, character_id);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferState {
    Receiving,
    Committed,
    Acknowledged,
}

impl TransferState {
    pub fn as_str(self) -> &'static str {
        match self {
            TransferState::Receiving => "receiving",
            TransferState::Committed => "committed",
            TransferState::Acknowledged => "acknowledged",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "committed" => TransferState::Committed,
            "acknowledged" => TransferState::Acknowledged,
            _ => TransferState::Receiving,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostTransfer {
    pub realm_local_id: String,
    pub transfer_id: Uuid,
    pub character_id: Uuid,
    pub player_id: Uuid,
    pub revision: u64,
    pub content_hash: String,
    pub total_size: usize,
    pub received_size: usize,
    pub state: TransferState,
    pub local_guid: Option<u32>,
    pub session_id: Option<Uuid>,
    pub projected_level: Option<u32>,
    pub notes: Vec<String>,
    pub collections: BTreeMap<String, Vec<u32>>,
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

    pub fn host_claim_of_character(&self, realm: &str, character: &Uuid) -> Result<Option<HostClaim>> {
        self.conn
            .query_row("SELECT local_guid, character_id, player_id, state FROM host_claim WHERE realm_local_id = ?1 AND character_id = ?2 LIMIT 1", params![realm, character.to_string()], row_claim)
            .optional()
            .map_err(db)?
            .transpose()
    }

    pub fn host_transfer(&self, realm: &str, transfer_id: &Uuid) -> Result<Option<HostTransfer>> {
        self.conn
            .query_row(
                "SELECT realm_local_id, transfer_id, character_id, player_id, revision, content_hash, total_size, received_size, state, local_guid, session_id, projected_level, notes, collections FROM host_transfer WHERE realm_local_id = ?1 AND transfer_id = ?2",
                params![realm, transfer_id.to_string()],
                row_transfer,
            )
            .optional()
            .map_err(db)?
            .transpose()
    }

    pub fn host_transfer_latest(&self, realm: &str, character_id: &Uuid) -> Result<Option<HostTransfer>> {
        self.conn
            .query_row(
                "SELECT realm_local_id, transfer_id, character_id, player_id, revision, content_hash, total_size, received_size, state, local_guid, session_id, projected_level, notes, collections FROM host_transfer WHERE realm_local_id = ?1 AND character_id = ?2 ORDER BY updated_at DESC LIMIT 1",
                params![realm, character_id.to_string()],
                row_transfer,
            )
            .optional()
            .map_err(db)?
            .transpose()
    }

    pub fn host_transfer_save(&self, t: &HostTransfer) -> Result<()> {
        let notes_json = serde_json::to_string(&t.notes).unwrap_or_default();
        let coll_json = serde_json::to_string(&t.collections).unwrap_or_default();
        self.conn
            .execute(
                "INSERT INTO host_transfer(realm_local_id, transfer_id, character_id, player_id, revision, content_hash, total_size, received_size, state, local_guid, session_id, projected_level, notes, collections, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
                 ON CONFLICT(transfer_id) DO UPDATE SET
                   received_size = excluded.received_size,
                   state = excluded.state,
                   local_guid = excluded.local_guid,
                   session_id = excluded.session_id,
                   projected_level = excluded.projected_level,
                   notes = excluded.notes,
                   collections = excluded.collections,
                   updated_at = excluded.updated_at",
                params![
                    t.realm_local_id,
                    t.transfer_id.to_string(),
                    t.character_id.to_string(),
                    t.player_id.to_string(),
                    t.revision as i64,
                    t.content_hash,
                    t.total_size as i64,
                    t.received_size as i64,
                    t.state.as_str(),
                    t.local_guid,
                    t.session_id.map(|s| s.to_string()),
                    t.projected_level,
                    notes_json,
                    coll_json,
                    now(),
                    now(),
                ],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn host_transfer_update_progress(&self, transfer_id: &Uuid, received: usize) -> Result<()> {
        self.conn
            .execute("UPDATE host_transfer SET received_size = ?1, updated_at = ?2 WHERE transfer_id = ?3", params![received as i64, now(), transfer_id.to_string()])
            .map_err(db)?;
        Ok(())
    }

    pub fn host_transfer_commit(&self, transfer_id: &Uuid, local_guid: u32, session_id: &Uuid, projected_level: Option<u32>, notes: &[String]) -> Result<()> {
        let notes_json = serde_json::to_string(notes).unwrap_or_default();
        self.conn
            .execute(
                "UPDATE host_transfer SET state = 'committed', local_guid = ?1, session_id = ?2, projected_level = ?3, notes = ?4, updated_at = ?5 WHERE transfer_id = ?6",
                params![local_guid, session_id.to_string(), projected_level, notes_json, now(), transfer_id.to_string()],
            )
            .map_err(db)?;
        Ok(())
    }

    pub fn host_transfer_ack(&self, transfer_id: &Uuid) -> Result<bool> {
        let n = self.conn.execute("UPDATE host_transfer SET state = 'acknowledged', updated_at = ?1 WHERE transfer_id = ?2", params![now(), transfer_id.to_string()]).map_err(db)?;
        Ok(n == 1)
    }

    pub fn host_transfer_delete(&self, transfer_id: &Uuid) -> Result<()> {
        self.conn.execute("DELETE FROM host_transfer WHERE transfer_id = ?1", params![transfer_id.to_string()]).map_err(db)?;
        Ok(())
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

fn row_transfer(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<HostTransfer>> {
    let realm_local_id: String = r.get(0)?;
    let tid: String = r.get(1)?;
    let cid: String = r.get(2)?;
    let pid: String = r.get(3)?;
    let revision: i64 = r.get(4)?;
    let content_hash: String = r.get(5)?;
    let total_size: i64 = r.get(6)?;
    let received_size: i64 = r.get(7)?;
    let state_str: String = r.get(8)?;
    let local_guid: Option<u32> = r.get(9)?;
    let session_str: Option<String> = r.get(10)?;
    let projected_level: Option<u32> = r.get(11)?;
    let notes_str: Option<String> = r.get(12)?;
    let notes = notes_str.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()).unwrap_or_default();
    let coll_str: Option<String> = r.get(13)?;
    let collections = coll_str.and_then(|s| serde_json::from_str::<BTreeMap<String, Vec<u32>>>(&s).ok()).unwrap_or_default();
    Ok((|| {
        Ok(HostTransfer {
            realm_local_id,
            transfer_id: uuid(tid)?,
            character_id: uuid(cid)?,
            player_id: uuid(pid)?,
            revision: revision as u64,
            content_hash,
            total_size: total_size as usize,
            received_size: received_size as usize,
            state: TransferState::parse(&state_str),
            local_guid,
            session_id: session_str.map(uuid).transpose()?,
            projected_level,
            notes,
            collections,
        })
    })())
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

    #[test]
    fn host_transfer_roundtrip_and_lifecycle() {
        let s = ControlStore::open_in_memory().unwrap();
        let (p, c, t, sid) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let transfer = HostTransfer {
            realm_local_id: "live".into(),
            transfer_id: t,
            character_id: c,
            player_id: p,
            revision: 3,
            content_hash: "a".repeat(64),
            total_size: 4096,
            received_size: 0,
            state: TransferState::Receiving,
            local_guid: None,
            session_id: None,
            projected_level: None,
            notes: vec![],
            collections: BTreeMap::from([("coa:appearance".into(), vec![1, 2, 3])]),
        };
        s.host_transfer_save(&transfer).unwrap();
        let got = s.host_transfer("live", &t).unwrap().unwrap();
        assert_eq!(got.state, TransferState::Receiving);
        assert_eq!(got.total_size, 4096);

        s.host_transfer_update_progress(&t, 2048).unwrap();
        let got = s.host_transfer("live", &t).unwrap().unwrap();
        assert_eq!(got.received_size, 2048);

        s.host_transfer_commit(&t, 101, &sid, Some(60), &["projected 60".into()]).unwrap();
        let got = s.host_transfer("live", &t).unwrap().unwrap();
        assert_eq!(got.state, TransferState::Committed);
        assert_eq!(got.local_guid, Some(101));
        assert_eq!(got.session_id, Some(sid));
        assert_eq!(got.projected_level, Some(60));

        let latest = s.host_transfer_latest("live", &c).unwrap().unwrap();
        assert_eq!(latest.transfer_id, t);

        assert!(s.host_transfer_ack(&t).unwrap());
        let got = s.host_transfer("live", &t).unwrap().unwrap();
        assert_eq!(got.state, TransferState::Acknowledged);
    }
}
