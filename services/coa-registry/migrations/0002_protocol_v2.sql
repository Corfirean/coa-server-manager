-- Registry schema, migration 2 (protocol 2: the structured listing). Forward-only; every realm keeps its id, its key, its creation time and its capabilities.
-- A record written under protocol 1 has advert_version = 1: it is kept, and it is not shown in a public list until its Host republishes under protocol 2.
ALTER TABLE realms RENAME COLUMN player_count TO players;
ALTER TABLE realms RENAME COLUMN player_capacity TO capacity;
ALTER TABLE realms
    ADD COLUMN bots                  integer  NOT NULL DEFAULT 0 CHECK (bots BETWEEN 0 AND 100000),
    ADD COLUMN region                text     CHECK (region IS NULL OR char_length(region) BETWEEN 2 AND 16),
    ADD COLUMN rates                 jsonb    NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(rates) = 'object' AND octet_length(rates::text) <= 1024),
    ADD COLUMN modules               jsonb    NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(modules) = 'array' AND octet_length(modules::text) <= 16384),
    ADD COLUMN account_automatic     boolean  NOT NULL DEFAULT false,
    ADD COLUMN account_existing_only boolean  NOT NULL DEFAULT true,
    ADD COLUMN listing_hash          text     CHECK (listing_hash IS NULL OR listing_hash ~ '^[0-9a-f]{64}$'),
    ADD COLUMN level_cap             integer  CHECK (level_cap IS NULL OR level_cap BETWEEN 1 AND 255),
    ADD COLUMN advert_version        smallint NOT NULL DEFAULT 1 CHECK (advert_version IN (1, 2));
-- the level cap of a protocol-1 record is in the capabilities it already holds
UPDATE realms SET level_cap = (capabilities -> 'progression' ->> 'max_player_level')::integer
 WHERE jsonb_typeof(capabilities -> 'progression') = 'object' AND (capabilities -> 'progression' ->> 'max_player_level') ~ '^[0-9]{1,3}$';
