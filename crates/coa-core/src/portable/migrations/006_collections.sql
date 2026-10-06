-- Schema 6: account collections of the HOST (Phase 6). What the Host has read from a realm account and what the Owner has
-- acknowledged of it. Nothing here is canonical: the Owner's `collection` table (revision + hash + compact id set per profile
-- and kind) is. A realm is asked for its sets again only when the cheap `fingerprint` of its tables changes.

CREATE TABLE host_collection (
    server_id          TEXT NOT NULL,
    account            INTEGER NOT NULL,
    kind               TEXT NOT NULL,
    -- count / max / checksum of the realm's rows at the last full read: equal fingerprint means nothing to read
    fingerprint        TEXT NOT NULL,
    -- hash of the set read at that time
    observed_hash      BLOB CHECK (observed_hash IS NULL OR length(observed_hash) = 32),
    -- hash of the realm set the Owner has acknowledged (a realm set whose hash equals this one is never sent again)
    acked_hash         BLOB CHECK (acked_hash IS NULL OR length(acked_hash) = 32),
    canonical_revision INTEGER NOT NULL DEFAULT 0,
    canonical_hash     BLOB CHECK (canonical_hash IS NULL OR length(canonical_hash) = 32),
    -- the CollectionObserved message not yet acknowledged
    pending            BLOB,
    checked_at         INTEGER NOT NULL DEFAULT 0,
    updated_at         TEXT NOT NULL,
    PRIMARY KEY (server_id, account, kind)
) STRICT;
