-- Phase-10 gate item 17: what the database holds. Run: sudo sh audit-db.sh
\echo == tables
SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY 1;
\echo == columns of realms
SELECT column_name || ' ' || data_type FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'realms' ORDER BY ordinal_position;
\echo == any table or column that looks like players, characters, accounts, credentials, snapshots (must be empty)
SELECT table_name || '.' || column_name FROM information_schema.columns
 WHERE table_schema = 'public' AND (column_name ~* '(password|passwd|secret|token|credential|account|character|snapshot|inventory|payload|session|canonical|guid|ra_user|mysql)' OR table_name ~* '(player|character|account|session|snapshot|credential)');
\echo == rows containing forbidden words anywhere in their text form (must be 0)
SELECT count(*) FROM realms r WHERE row_to_json(r)::text ~* '(password|credential|secret|snapshot|inventory|canonical|character_id|ra_password|mysql|private)';
\echo == the keys stored are public keys of 32 bytes (private keys are 32 bytes too: they are never sent, there is no field for them)
SELECT count(*) AS realms, count(*) FILTER (WHERE octet_length(public_key) = 32) AS keys32 FROM realms;
\echo == largest stored capabilities (bytes)
SELECT max(octet_length(capabilities::text)) FROM realms;
