//! What the Manager remembers of level-cap projections (Phase 8): the context of each projected character on each realm, and the
//! progression signature each character was last synchronised under.

use rusqlite::{params, OptionalExtension};

use super::*;
use crate::portable::projection::{ProgressionPin, ProjectionContext};

impl Store {
    /// Remember (replace) the projection of a character on a realm. The character must be bound to the realm.
    pub fn set_projection_context(&mut self, id: CharacterId, server_id: &str, ctx: &ProjectionContext) -> Result<()> {
        check_server_id(server_id)?;
        ctx.validate()?;
        let json = serde_json::to_string(ctx)?;
        let tx = self.write_tx()?;
        let bound: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2)", params![id.to_string(), server_id], |r| r.get(0))?;
        if !bound {
            return Err(PortableError::Invalid(format!("character {id} is not on realm {server_id}")));
        }
        tx.execute(
            "INSERT INTO realm_projection(character_id, server_id, canonical_level, projected_level, canonical_revision, content_profile_hash, progression_signature, policy_version, context, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(character_id, server_id) DO UPDATE SET canonical_level = excluded.canonical_level, projected_level = excluded.projected_level, canonical_revision = excluded.canonical_revision,
                content_profile_hash = excluded.content_profile_hash, progression_signature = excluded.progression_signature, policy_version = excluded.policy_version, context = excluded.context, updated_at = excluded.updated_at",
            params![id.to_string(), server_id, ctx.canonical_level, ctx.projected_level, ctx.canonical_revision as i64, ctx.content_profile_hash, ctx.progression_signature, ctx.projection_policy_version, json, now()],
        )?;
        tx.execute("UPDATE character_server_mapping SET progression_pin = ?3 WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id, serde_json::to_string(&ctx.pin())?])?;
        tx.commit()?;
        Ok(())
    }

    pub fn projection_context(&self, id: CharacterId, server_id: &str) -> Result<Option<ProjectionContext>> {
        let json: Option<String> = self.conn.query_row("SELECT context FROM realm_projection WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id], |r| r.get(0)).optional()?;
        json.map(|j| {
            let ctx: ProjectionContext = serde_json::from_str(&j).map_err(|e| PortableError::CorruptSnapshot(format!("a stored projection context is not valid: {e}")))?;
            ctx.validate()?;
            Ok(ctx)
        })
        .transpose()
    }

    /// The character is not projected on this realm (any more).
    pub fn clear_projection_context(&mut self, id: CharacterId, server_id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM realm_projection WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id])? > 0)
    }

    /// Stop managing a character's copy on a realm: the mapping, the item and pet mappings, the synchronised snapshot, the open baseline and the projection
    /// are forgotten; the realm's own character is left exactly as it is. Live sessions of it are closed. `false`: it was not on that realm.
    pub fn detach_realm_copy(&mut self, id: CharacterId, server_id: &str) -> Result<bool> {
        check_server_id(server_id)?;
        let tx = self.write_tx()?;
        let at = now();
        let removed = tx.execute("DELETE FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id])?;
        for sql in [
            "DELETE FROM realm_projection WHERE character_id = ?1 AND server_id = ?2",
            "DELETE FROM realm_extension_state WHERE character_id = ?1 AND server_id = ?2",
            "UPDATE realm_baseline SET state = 'closed', updated_at = ?3 WHERE character_id = ?1 AND server_id = ?2 AND state = 'open'",
            "UPDATE host_session SET state = 'closed', updated_at = ?3 WHERE character_id = ?1 AND server_id = ?2 AND state IN ('armed', 'open')",
            "UPDATE owner_session SET state = 'superseded', updated_at = ?3 WHERE character_id = ?1 AND server_id = ?2 AND state IN ('offered', 'open')",
        ] {
            if sql.contains("?3") {
                tx.execute(sql, params![id.to_string(), server_id, at])?;
            } else {
                tx.execute(sql, params![id.to_string(), server_id])?;
            }
        }
        tx.commit()?;
        Ok(removed > 0)
    }

    /// The working copy was brought to a newer canonical revision (an acknowledged checkpoint): the decision stays, the revision moves.
    pub fn advance_projection_revision(&mut self, id: CharacterId, server_id: &str, revision: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE realm_projection SET canonical_revision = ?3, context = json_set(context, '$.canonical_revision', ?3), updated_at = ?4 WHERE character_id = ?1 AND server_id = ?2",
            params![id.to_string(), server_id, revision as i64, now()],
        )?;
        Ok(())
    }

    /// Record the pin a character was synchronised under on a realm (`None`: unknown, a realm that did not report its progression).
    pub fn set_mapping_pin(&mut self, id: CharacterId, server_id: &str, pin: Option<&ProgressionPin>) -> Result<()> {
        if let Some(p) = pin {
            p.validate()?;
        }
        let json = pin.map(serde_json::to_string).transpose()?;
        let n = self.conn.execute("UPDATE character_server_mapping SET progression_pin = ?3 WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id, json])?;
        if n == 0 {
            return Err(PortableError::Invalid(format!("character {id} is not on realm {server_id}")));
        }
        Ok(())
    }

    /// The pin a character on a realm was last synchronised under (projected or native).
    pub fn character_pin(&self, id: CharacterId, server_id: &str) -> Result<Option<ProgressionPin>> {
        let json: Option<String> = self
            .conn
            .query_row("SELECT progression_pin FROM character_server_mapping WHERE character_id = ?1 AND server_id = ?2", params![id.to_string(), server_id], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten();
        json.map(|j| {
            let pin: ProgressionPin = serde_json::from_str(&j).map_err(|e| PortableError::CorruptSnapshot(format!("a stored progression pin is not valid: {e}")))?;
            pin.validate()?;
            Ok(pin)
        })
        .transpose()
    }
}
