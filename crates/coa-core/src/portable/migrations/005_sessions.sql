-- Schema 5: runtime portable sessions (Phase 5). The OWNER side and the HOST side keep their own tables; one Manager may
-- play both roles on one file in a prototype, but a deployment uses two files and the roles never read each other's.
--
--  owner_session / owner_checkpoint : what the Owner has offered, started and applied for one session of one character on one
--                                     realm. C0 and B0 are kept as sealed snapshots (the anchors of every checkpoint merge) and
--                                     every applied sequence is logged (idempotency, ordering, lost acknowledgements).
--  host_session / host_outbox       : what the Host has armed on its realm, the sequence it is at, the state it has already
--                                     read from the realm, and the messages not yet acknowledged.

CREATE TABLE owner_session (
    session_id          TEXT PRIMARY KEY,
    character_id        TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    server_id           TEXT NOT NULL,
    state               TEXT NOT NULL CHECK (state IN ('offered', 'open', 'closed', 'superseded')),
    c0_revision         INTEGER NOT NULL,
    c0_hash             BLOB NOT NULL CHECK (length(c0_hash) = 32),
    c0_payload          BLOB NOT NULL,
    baseline_generation INTEGER,
    b0_hash             BLOB CHECK (b0_hash IS NULL OR length(b0_hash) = 32),
    b0_payload          BLOB,
    -- the canonical revision this session produced last (starts at c0_revision); the next checkpoint must still find it
    head_revision       INTEGER NOT NULL,
    last_sequence       INTEGER NOT NULL DEFAULT 0,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    CHECK (state NOT IN ('open', 'closed') OR b0_hash IS NOT NULL)
) STRICT;
CREATE UNIQUE INDEX owner_session_one_live ON owner_session(character_id, server_id) WHERE state IN ('offered', 'open');

CREATE TABLE owner_checkpoint (
    session_id         TEXT NOT NULL REFERENCES owner_session(session_id) ON DELETE CASCADE,
    sequence           INTEGER NOT NULL,
    content_hash       BLOB NOT NULL CHECK (length(content_hash) = 32),
    resulting_revision INTEGER NOT NULL,
    final              INTEGER NOT NULL CHECK (final IN (0, 1)),
    next_session_id    TEXT,
    applied_at         TEXT NOT NULL,
    PRIMARY KEY (session_id, sequence)
) STRICT;

CREATE TABLE host_session (
    session_id        TEXT PRIMARY KEY,
    character_id      TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    server_id         TEXT NOT NULL,
    local_guid        INTEGER,
    base_revision     INTEGER NOT NULL,
    generation        INTEGER NOT NULL DEFAULT 1,
    -- armed: the realm holds the marker, B0 not read yet; open: B0 sent, checkpoints running; closed
    state             TEXT NOT NULL CHECK (state IN ('armed', 'open', 'closed')),
    -- the sequence the next checkpoint will use; persisted BEFORE the realm is asked to save
    next_sequence     INTEGER NOT NULL DEFAULT 1,
    pending_sequence  INTEGER,
    acked_sequence    INTEGER NOT NULL DEFAULT 0,
    owned_items       TEXT NOT NULL DEFAULT '[]',
    owned_pets        TEXT NOT NULL DEFAULT '[]',
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX host_session_one_live ON host_session(character_id, server_id) WHERE state IN ('armed', 'open');

CREATE TABLE host_outbox (
    session_id   TEXT NOT NULL REFERENCES host_session(session_id) ON DELETE CASCADE,
    sequence     INTEGER NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('started', 'checkpoint')),
    message      BLOB NOT NULL,
    state        TEXT NOT NULL CHECK (state IN ('pending', 'acked')),
    created_at   TEXT NOT NULL,
    PRIMARY KEY (session_id, sequence)
) STRICT;
