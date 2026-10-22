-- Registry schema, migration 3 (the public list). Forward-only. One partial index per sort, over exactly the rows a public list can show
-- (published, protocol 2) and exactly the expressions the list sorts by, so that a page is an index range scan with a LIMIT even with many thousands of realms.
CREATE INDEX realms_browse_name    ON realms ((lower(display_name)), realm_id)                    WHERE published AND advert_version = 2;
CREATE INDEX realms_browse_players ON realms ((COALESCE(players, 0)::bigint), realm_id)            WHERE published AND advert_version = 2;
CREATE INDEX realms_browse_cap     ON realms ((COALESCE(level_cap, 0)::bigint), realm_id)          WHERE published AND advert_version = 2;
CREATE INDEX realms_browse_created ON realms (created_at, realm_id)                                          WHERE published AND advert_version = 2;
CREATE INDEX realms_browse_modules ON realms USING gin (modules jsonb_path_ops)                     WHERE published AND advert_version = 2;
