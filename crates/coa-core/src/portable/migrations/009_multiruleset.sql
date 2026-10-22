-- Schema 9: Multi-ruleset partitioned collections and reborn ruleset support.
-- 1. Partition collection table by (profile_id, ruleset, kind)
CREATE TABLE collection_new (
    profile_id          TEXT NOT NULL REFERENCES profile(profile_id),
    ruleset             TEXT NOT NULL CHECK (ruleset IN ('coa', 'wildcard', 'reborn')),
    kind                TEXT NOT NULL,
    collection_revision INTEGER NOT NULL CHECK (collection_revision >= 1),
    collection_hash     BLOB NOT NULL CHECK (length(collection_hash) = 32),
    format_version      INTEGER NOT NULL,
    item_count          INTEGER NOT NULL,
    updated_at          TEXT NOT NULL,
    payload             BLOB NOT NULL,
    PRIMARY KEY (profile_id, ruleset, kind)
) STRICT;

INSERT INTO collection_new (profile_id, ruleset, kind, collection_revision, collection_hash, format_version, item_count, updated_at, payload)
SELECT profile_id, 'coa', kind, collection_revision, collection_hash, format_version, item_count, updated_at, payload
FROM collection;

DROP TABLE collection;
ALTER TABLE collection_new RENAME TO collection;

CREATE INDEX IF NOT EXISTS character_profile_ruleset ON character(profile_id, ruleset);
