-- Schema 4: what Phase 4 (reconciliation) needs to remember between sessions.
--
--  * item/pet mappings know whether the realm *holds* the item ("present"), has filtered it away (mailed, unknown entry,
--    cannot equip: "filtered", the canonical character still owns it) or whether it is the realm's own addition
--    ("realm_local", never merged into the canonical character);
--  * pets get stable per-realm mappings like items;
--  * `synced_*` keeps the canonical snapshot a realm was last synchronised with (the base of the next in-place update);
--  * `realm_baseline` keeps C0 (canonical at join) and B0 (the realm after its own first load/save normalisation, before
--    any progression): the base of the three-way reconciliation, persisted so it survives restarts and history pruning;
--  * the import journal also journals in-place updates of an existing realm character.

ALTER TABLE item_mapping ADD COLUMN presence TEXT NOT NULL DEFAULT 'present' CHECK (presence IN ('present', 'filtered', 'realm_local'));

CREATE TABLE pet_mapping (
    mapping_id         INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id       TEXT NOT NULL,
    server_id          TEXT NOT NULL,
    portable_pet_id    TEXT NOT NULL,
    local_pet_number   INTEGER NOT NULL,
    identity           TEXT NOT NULL,
    state              TEXT NOT NULL CHECK (state IN ('active', 'retired')),
    presence           TEXT NOT NULL DEFAULT 'present' CHECK (presence IN ('present', 'filtered', 'realm_local')),
    created_revision   INTEGER NOT NULL,
    confirmed_revision INTEGER NOT NULL,
    retired_revision   INTEGER,
    retired_reason     TEXT CHECK (retired_reason IS NULL OR retired_reason IN ('guid_reused', 'moved', 'absent', 'character_rebound')),
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL,
    CHECK ((state = 'active') = (retired_revision IS NULL)),
    FOREIGN KEY (character_id, server_id) REFERENCES character_server_mapping(character_id, server_id) ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX pet_mapping_active_pet ON pet_mapping(character_id, server_id, portable_pet_id) WHERE state = 'active';
CREATE UNIQUE INDEX pet_mapping_active_number ON pet_mapping(character_id, server_id, local_pet_number) WHERE state = 'active';

ALTER TABLE character_server_mapping ADD COLUMN synced_hash BLOB;
ALTER TABLE character_server_mapping ADD COLUMN synced_payload BLOB;

CREATE TABLE realm_baseline (
    character_id  TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    server_id     TEXT NOT NULL,
    c0_revision   INTEGER NOT NULL,
    -- the canonical revision this session has produced so far (starts at c0_revision); the next reconcile must still find it
    head_revision INTEGER NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('open', 'closed')),
    c0_hash       BLOB NOT NULL CHECK (length(c0_hash) = 32),
    c0_payload    BLOB NOT NULL,
    b0_hash       BLOB NOT NULL CHECK (length(b0_hash) = 32),
    b0_payload    BLOB NOT NULL,
    note          TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    PRIMARY KEY (character_id, server_id, c0_revision)
) STRICT;
CREATE UNIQUE INDEX baseline_one_open ON realm_baseline(character_id, server_id) WHERE state = 'open';

ALTER TABLE import_journal ADD COLUMN kind TEXT NOT NULL DEFAULT 'import' CHECK (kind IN ('import', 'update'));
-- ids of items / pets the update retires from the realm's mappings
ALTER TABLE import_journal ADD COLUMN retired_items TEXT NOT NULL DEFAULT '[]';
ALTER TABLE import_journal ADD COLUMN retired_pets TEXT NOT NULL DEFAULT '[]';
-- the canonical snapshot the realm is being synchronised with (zstd canonical JSON)
ALTER TABLE import_journal ADD COLUMN target_hash BLOB;
ALTER TABLE import_journal ADD COLUMN target_payload BLOB;
