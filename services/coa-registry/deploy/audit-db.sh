#!/bin/sh
# Run on the node as root: sudo sh audit-db.sh   (the password is read inside the container from its own secret)
set -eu
docker exec -i coa-postgres-postgres-1 sh -c 'PGPASSWORD=$(cat /run/secrets/postgres_password) exec psql -U coa -d coa_registry -X -q' < "$(dirname "$0")/audit-db.sql"
