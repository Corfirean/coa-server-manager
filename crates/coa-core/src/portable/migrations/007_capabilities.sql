-- Schema 7: realm capabilities (Phase 7).
--
--  realm_profile          the last content profile known of each realm (live from its core, or assembled offline), so an offline
--                         import or update can still be evaluated, and a change of it can be seen
--  character_server_mapping.content_profile_hash
--                         the content profile hash the realm had when this character was last synchronised with it; a different
--                         hash on the realm means what was held back for it must be looked at again, whatever the revision
--  realm_extension_state  the extension payloads (by content hash) that were applied to a character on a realm, so a payload
--                         is applied once and a changed one again

ALTER TABLE character_server_mapping ADD COLUMN content_profile_hash TEXT;

CREATE TABLE realm_profile (
    server_id            TEXT PRIMARY KEY,
    content_profile_hash TEXT NOT NULL,
    profile              TEXT NOT NULL,
    source               TEXT NOT NULL CHECK (source IN ('live', 'offline')),
    fetched_at           TEXT NOT NULL
) STRICT;

CREATE TABLE realm_extension_state (
    character_id TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    server_id    TEXT NOT NULL,
    namespace    TEXT NOT NULL,
    applied_hash TEXT NOT NULL,
    applied_at   TEXT NOT NULL,
    PRIMARY KEY (character_id, server_id, namespace)
) STRICT;
