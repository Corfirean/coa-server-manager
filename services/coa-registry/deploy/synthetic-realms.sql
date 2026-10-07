-- Phase-11 gate: thousands of synthetic realms, to see that the public list pages through them quickly. Marked so that they can be removed exactly.
-- Load:    sudo sh synthetic.sh load 5000        Remove: sudo sh synthetic.sh remove
-- (the psql variable :n is the number of realms)
INSERT INTO realms (realm_id, public_key, created_at, updated_at, last_seen_at, published, display_name, description, language, region, rates, modules,
                    account_automatic, account_existing_only, manager_version, listing_hash, ruleset, level_cap, capabilities, capabilities_hash,
                    players, bots, capacity, metadata_revision, last_request_ts, advert_version)
SELECT overlay(overlay(md5('syn' || i) placing '7' from 13) placing '8' from 17)::uuid,
       decode(md5('k' || i) || md5('l' || i), 'hex'),
       date_trunc('second', now() - (i || ' minutes')::interval), now(), now() - ((i % 15) || ' seconds')::interval, true,
       (ARRAY['Descension', 'Alpha Realm', 'bravo', 'Delta Force', 'Echo', 'Zulu Base', 'Ünïcode Réalm', 'Яндекс Мир'])[1 + i % 8] || ' ' || lpad(i::text, 5, '0'),
       'SYNTHETIC-GATE realm ' || i,
       (ARRAY['en', 'ru', 'de'])[1 + i % 3], (ARRAY['EU', 'NA', 'RU'])[1 + i % 3],
       jsonb_build_object('xp_kill', (1 + i % 5)::float8, 'xp_quest', NULL, 'xp_explore', NULL, 'loot', 1.0, 'money', NULL, 'reputation', NULL, 'honor', NULL),
       jsonb_build_array(jsonb_build_object('id', 'playerbots', 'version', NULL, 'enabled', i % 3 = 0), jsonb_build_object('id', 'content-scaling', 'version', NULL, 'enabled', i % 4 = 0)),
       i % 2 = 0, i % 2 = 1, '0.6.6', repeat('a', 64), 'coa', (ARRAY[60, 70, 80, 255])[1 + i % 4],
       (SELECT capabilities FROM realms WHERE advert_version = 2 AND description NOT LIKE 'SYNTHETIC-GATE%' LIMIT 1), repeat('b', 64),
       (i * 7) % 200, (i * 3) % 90, 200, 1, 0, 2
  FROM generate_series(1, :n) AS i;
ANALYZE realms;
