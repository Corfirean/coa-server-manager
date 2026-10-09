//! What the Manager remembers of the realms' content profiles (Phase 7): the last profile of each realm, the profile hash each
//! character was last synchronised under, and which extension payloads were applied where.

use std::collections::HashMap;

use rusqlite::{params, OptionalExtension};

use super::*;
use crate::portable::capabilities::RealmCapabilities;

/// Whether a stored realm profile changed when a new one was recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChange {
    pub previous: Option<String>,
    pub current: String,
    /// The progression signature the realm had before, and now (`None`: not known).
    pub previous_progression: Option<String>,
    pub current_progression: Option<String>,
}

impl ProfileChange {
    pub fn changed(&self) -> bool {
        self.previous.as_deref() != Some(self.current.as_str())
    }

    /// The realm's level cap or progression rules are not the ones it had when it was last looked at.
    pub fn progression_changed(&self) -> bool {
        self.previous.is_some() && self.previous_progression != self.current_progression
    }
}

/// A character whose synchronised profile is not the realm's current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleMapping {
    pub character_id: CharacterId,
    pub local_guid: u32,
    /// `None`: synchronised before profiles existed.
    pub synced_profile: Option<String>,
}

impl Store {
    /// Remember a realm's profile. The previous hash is returned so the caller can tell that the realm changed.
    pub fn set_realm_profile(
        &mut self,
        server_id: &str,
        caps: &RealmCapabilities,
        source: &str,
    ) -> Result<ProfileChange> {
        check_server_id(server_id)?;
        if !matches!(source, "live" | "offline") {
            return Err(PortableError::Invalid(
                "a profile source is live or offline".into(),
            ));
        }
        caps.validate()?;
        let json = String::from_utf8(caps.to_json()?)
            .map_err(|_| PortableError::Invalid("the profile is not UTF-8".into()))?;
        let tx = self.write_tx()?;
        let previous_row: Option<(String, String)> = tx
            .query_row(
                "SELECT content_profile_hash, profile FROM realm_profile WHERE server_id = ?1",
                [server_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let previous = previous_row.as_ref().map(|(h, _)| h.clone());
        let previous_progression = previous_row
            .and_then(|(_, json)| RealmCapabilities::from_json(json.as_bytes()).ok())
            .and_then(|c| c.progression)
            .map(|p| p.progression_signature);
        tx.execute(
            "INSERT INTO realm_profile(server_id, content_profile_hash, profile, source, fetched_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(server_id) DO UPDATE SET content_profile_hash = excluded.content_profile_hash, profile = excluded.profile, source = excluded.source, fetched_at = excluded.fetched_at",
            params![server_id, caps.content_profile_hash, json, source, now()],
        )?;
        tx.commit()?;
        Ok(ProfileChange {
            previous,
            current: caps.content_profile_hash.clone(),
            previous_progression,
            current_progression: caps
                .progression
                .as_ref()
                .map(|p| p.progression_signature.clone()),
        })
    }

    pub fn realm_profile(&self, server_id: &str) -> Result<Option<RealmCapabilities>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT profile FROM realm_profile WHERE server_id = ?1",
                [server_id],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|j| RealmCapabilities::from_json(j.as_bytes()))
            .transpose()
    }

    /// Record the profile hash a character was synchronised under on a realm.
    pub fn set_mapping_profile(
        &mut self,
        id: CharacterId,
        server_id: &str,
        hash: &str,
    ) -> Result<()> {
        let n = self.conn.execute("UPDATE character_server_mapping SET content_profile_hash = ?3 WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id, hash])?;
        if n == 0 {
            return Err(PortableError::Invalid(format!(
                "character {id} is not on realm {server_id}"
            )));
        }
        Ok(())
    }

    pub fn mapping_profile(&self, id: CharacterId, server_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT content_profile_hash FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    }

    /// The characters of a realm whose synchronised profile is not `hash`.
    pub fn mappings_with_other_profile(
        &self,
        server_id: &str,
        hash: &str,
    ) -> Result<Vec<StaleMapping>> {
        let mut stmt = self
            .conn
            .prepare("SELECT character_id, local_guid, content_profile_hash FROM character_server_mapping WHERE server_id = ?1 AND (content_profile_hash IS NULL OR content_profile_hash <> ?2) ORDER BY character_id")?;
        let rows = stmt.query_map(params![server_id, hash], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        rows.map(|r| {
            let (id, guid, profile) = r?;
            Ok(StaleMapping {
                character_id: id.parse()?,
                local_guid: guid as u32,
                synced_profile: profile,
            })
        })
        .collect()
    }

    /// The extension payloads applied to a character on a realm: namespace -> content hash of what was applied.
    pub fn extension_state(
        &self,
        id: CharacterId,
        server_id: &str,
    ) -> Result<HashMap<String, String>> {
        let mut stmt = self.conn.prepare("SELECT namespace, applied_hash FROM realm_extension_state WHERE character_id = ?1 AND server_id = ?2")?;
        let rows = stmt.query_map(params![id.to_string(), server_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<HashMap<_, _>, _>>()?)
    }

    pub fn set_extension_applied(
        &mut self,
        id: CharacterId,
        server_id: &str,
        namespace: &str,
        hash: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO realm_extension_state(character_id, server_id, namespace, applied_hash, applied_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(character_id, server_id, namespace) DO UPDATE SET applied_hash = excluded.applied_hash, applied_at = excluded.applied_at",
            params![id.to_string(), server_id, namespace, hash, now()],
        )?;
        Ok(())
    }

    /// Forget that a namespace was applied (the payload left the character, or the realm lost the module).
    pub fn clear_extension_applied(
        &mut self,
        id: CharacterId,
        server_id: &str,
        namespace: &str,
    ) -> Result<()> {
        self.conn.execute("DELETE FROM realm_extension_state WHERE character_id = ?1 AND server_id = ?2 AND namespace = ?3", params![id.to_string(), server_id, namespace])?;
        Ok(())
    }
}
