-- Schema 2: item mappings get a lifecycle and a content identity.
--
-- A realm hands out item guids from a counter that restarts at MAX(guid)+1, so a guid freed by a deleted item can
-- be given to a different one. A bare guid -> portable item mapping would then attach the new item to the old
-- portable id. Now every mapping records what the item *was* (`entry`, `identity`) when it was mapped, an old
-- mapping is retired (kept for the record) instead of overwritten, and only one *active* mapping may exist per
-- portable item and per local guid.

CREATE TABLE item_mapping_new (
    mapping_id         INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id       TEXT NOT NULL,
    server_id          TEXT NOT NULL,
    portable_item_id   TEXT NOT NULL,
    local_item_guid    INTEGER NOT NULL,
    entry              TEXT NOT NULL,
    identity           TEXT NOT NULL,
    state              TEXT NOT NULL CHECK (state IN ('active', 'retired')),
    created_revision   INTEGER NOT NULL,
    confirmed_revision INTEGER NOT NULL,
    retired_revision   INTEGER,
    retired_reason     TEXT CHECK (retired_reason IS NULL OR retired_reason IN ('guid_reused', 'moved', 'absent', 'character_rebound')),
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    CHECK ((state = 'active') = (retired_revision IS NULL)),
    FOREIGN KEY (character_id, server_id) REFERENCES character_server_mapping(character_id, server_id) ON DELETE CASCADE
) STRICT;

-- Schema-1 rows have no recorded identity: '' never matches a real identity, so they are re-verified, not trusted.
INSERT INTO item_mapping_new(character_id, server_id, portable_item_id, local_item_guid, entry, identity, state,
                             created_revision, confirmed_revision, created_at, updated_at)
SELECT character_id, server_id, portable_item_id, local_item_guid, '', '', 'active', 1, 1,
       strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
FROM item_mapping;

DROP TABLE item_mapping;
ALTER TABLE item_mapping_new RENAME TO item_mapping;

-- UNIQUE(character_id, server_id, local_item_guid) and one active mapping per portable item - for active rows.
CREATE UNIQUE INDEX item_mapping_active_item ON item_mapping(character_id, server_id, portable_item_id) WHERE state = 'active';
CREATE UNIQUE INDEX item_mapping_active_guid ON item_mapping(character_id, server_id, local_item_guid) WHERE state = 'active';
CREATE INDEX item_mapping_lookup ON item_mapping(character_id, server_id, state);
