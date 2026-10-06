-- Schema 3: the import journal. An import changes two systems that cannot share one transaction: the realm's MySQL and
-- this local store. The journal is the bridge: a row is written BEFORE the realm transaction ("prepared"), the realm
-- transaction stores a marker that carries the same nonce, and the row is completed in the same local transaction that
-- records the realm mappings. After a crash between the two commits, the marker in the realm decides what happened.

CREATE TABLE import_journal (
    import_id    TEXT PRIMARY KEY,
    character_id TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    server_id    TEXT NOT NULL,
    revision     INTEGER NOT NULL,
    -- the text of the realm's `coa.portable.import` settings row: nonce words and revision
    marker       TEXT NOT NULL,
    state        TEXT NOT NULL CHECK (state IN ('prepared', 'committed', 'aborted', 'needs_attention')),
    -- JSON, in plan order: item i is realm guid item_base + i, pet i is pet_base + i. Items carry what the mapping needs
    -- ({id, entry, identity}) so completing the import never depends on a snapshot that may have been pruned meanwhile.
    items        TEXT NOT NULL,
    pet_ids      TEXT NOT NULL,
    local_guid   INTEGER,
    item_base    INTEGER,
    pet_base     INTEGER,
    detail       TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
) STRICT;

-- one unfinished import per (character, realm)
CREATE UNIQUE INDEX import_one_open ON import_journal(character_id, server_id) WHERE state = 'prepared';
CREATE INDEX import_by_server ON import_journal(server_id, state);
