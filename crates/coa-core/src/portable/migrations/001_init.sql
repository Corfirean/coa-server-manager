-- Portable store, schema 1. Local to this Manager; realm databases are never touched.

CREATE TABLE profile (
    profile_id     TEXT PRIMARY KEY,
    created_at     TEXT NOT NULL,
    format_version INTEGER NOT NULL
) STRICT;

CREATE TABLE character (
    character_id TEXT PRIMARY KEY,
    profile_id   TEXT NOT NULL REFERENCES profile(profile_id),
    ruleset      TEXT NOT NULL CHECK (ruleset IN ('coa', 'wildcard')),
    name         TEXT NOT NULL,
    race         TEXT NOT NULL,
    class        TEXT NOT NULL,
    gender       INTEGER NOT NULL,
    level        INTEGER NOT NULL,
    revision     INTEGER NOT NULL CHECK (revision >= 1),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    archived     INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1))
) STRICT;
CREATE INDEX character_profile ON character(profile_id);

-- One row per kept revision. The newest row is the canonical state (character.revision points at it).
CREATE TABLE snapshot (
    character_id            TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    revision                INTEGER NOT NULL CHECK (revision >= 1),
    snapshot_format_version INTEGER NOT NULL,
    created_at              TEXT NOT NULL,
    source_server_id        TEXT NOT NULL,
    content_hash            BLOB NOT NULL CHECK (length(content_hash) = 32),
    uncompressed_size       INTEGER NOT NULL,
    payload                 BLOB NOT NULL,
    note                    TEXT,
    PRIMARY KEY (character_id, revision)
) STRICT, WITHOUT ROWID;

-- Which local guid a portable character has on which realm. Internal lookups (checkpoints, imports) go through
-- (server_id, local_guid): a local guid belongs to at most one portable character.
CREATE TABLE character_server_mapping (
    character_id  TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    server_id     TEXT NOT NULL,
    local_guid    INTEGER NOT NULL,
    last_revision INTEGER NOT NULL,
    state         TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (character_id, server_id),
    UNIQUE (server_id, local_guid)
) STRICT;

CREATE TABLE item_mapping (
    character_id     TEXT NOT NULL,
    server_id        TEXT NOT NULL,
    portable_item_id TEXT NOT NULL,
    local_item_guid  INTEGER NOT NULL,
    PRIMARY KEY (character_id, server_id, portable_item_id),
    UNIQUE (character_id, server_id, local_item_guid),
    FOREIGN KEY (character_id, server_id) REFERENCES character_server_mapping(character_id, server_id) ON DELETE CASCADE
) STRICT;

CREATE TABLE collection (
    profile_id          TEXT NOT NULL REFERENCES profile(profile_id),
    kind                TEXT NOT NULL,
    collection_revision INTEGER NOT NULL CHECK (collection_revision >= 1),
    collection_hash     BLOB NOT NULL CHECK (length(collection_hash) = 32),
    format_version      INTEGER NOT NULL,
    item_count          INTEGER NOT NULL,
    updated_at          TEXT NOT NULL,
    payload             BLOB NOT NULL,
    PRIMARY KEY (profile_id, kind)
) STRICT;

CREATE TABLE setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
