#!/bin/sh
# Phase-11 gate helper, run on the node as root:  sudo sh synthetic.sh load 5000 | remove | explain
set -eu
psqlc() { docker exec -i coa-postgres-postgres-1 sh -c 'PGPASSWORD=$(cat /run/secrets/postgres_password) exec psql -U coa -d coa_registry -X -q -v ON_ERROR_STOP=1 "$@"' sh "$@"; }
case "${1:-}" in
  load)   psqlc -v n="${2:-5000}" < "$(dirname "$0")/synthetic-realms.sql"; echo "loaded ${2:-5000} synthetic realms" ;;
  remove) echo "DELETE FROM realms WHERE description LIKE 'SYNTHETIC-GATE%'; ANALYZE realms; SELECT count(*) AS remaining FROM realms;" | psqlc ;;
  explain)
    for sort in "lower(display_name)" "COALESCE(players, 0)::bigint DESC" "COALESCE(level_cap, 0)::bigint" "created_at DESC"; do
      echo "== ORDER BY $sort"
      echo "EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, SUMMARY ON) SELECT realm_id FROM realms WHERE published AND advert_version = 2 AND last_seen_at > now() - interval '120 seconds' ORDER BY $sort, realm_id LIMIT 51;" | psqlc | sed -n '1,12p'
    done ;;
  *) echo "usage: synthetic.sh load [n] | remove | explain"; exit 2 ;;
esac
