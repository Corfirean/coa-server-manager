-- Registry schema, migration 1. Forward-only: a released migration is never edited.
-- Discovery and presence data only. There is deliberately no table for players, characters, accounts, sessions or credentials.
CREATE TABLE realms (
    realm_id          uuid PRIMARY KEY,
    public_key        bytea       NOT NULL CHECK (octet_length(public_key) = 32),
    created_at        timestamptz NOT NULL,
    updated_at        timestamptz NOT NULL,
    last_seen_at      timestamptz NOT NULL,
    published         boolean     NOT NULL,
    display_name      text        NOT NULL CHECK (char_length(display_name) BETWEEN 1 AND 80),
    description       text        NOT NULL CHECK (octet_length(description) <= 1024),
    language          text        NOT NULL CHECK (char_length(language) BETWEEN 2 AND 16),
    ruleset           text        NOT NULL CHECK (ruleset IN ('coa', 'wildcard')),
    manager_version   text        NOT NULL CHECK (char_length(manager_version) BETWEEN 1 AND 32),
    capabilities      jsonb       NOT NULL CHECK (octet_length(capabilities::text) <= 65536),
    capabilities_hash text        NOT NULL CHECK (capabilities_hash ~ '^[0-9a-f]{64}$'),
    metadata_revision bigint      NOT NULL CHECK (metadata_revision >= 1),
    -- the timestamp of the last signed request that was applied; the next one must be strictly later (replay protection)
    last_request_ts   bigint      NOT NULL,
    player_count      integer     CHECK (player_count BETWEEN 0 AND 100000),
    player_capacity   integer     CHECK (player_capacity BETWEEN 0 AND 100000)
);
CREATE INDEX realms_published_seen ON realms (published, last_seen_at);
