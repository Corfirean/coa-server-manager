-- Registry schema, migration 4. The public list's keyset cursor carries a creation time in whole seconds, so a row's creation time must be one:
-- every write path already stores it with `to_timestamp(<bigint>)`; this makes any other writer fail instead of silently repeating or skipping a realm in a page walk.
ALTER TABLE realms ADD CONSTRAINT realms_created_whole_seconds CHECK (created_at = date_trunc('second', created_at));
