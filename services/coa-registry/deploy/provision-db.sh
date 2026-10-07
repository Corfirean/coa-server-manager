#!/bin/sh
# Create the Registry's database role and database (idempotent). Run on the node as root: sudo sh provision-db.sh
# The password is read from /opt/coa/secrets/registry_db_password and goes to psql on stdin, never on a command line.
set -eu
PW=$(cat /opt/coa/secrets/registry_db_password)
docker exec -i coa-postgres-postgres-1 sh -c 'PGPASSWORD=$(cat /run/secrets/postgres_password) exec psql -U coa -d coa -v ON_ERROR_STOP=1 -q' <<SQL
DO \$\$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'coa_registry') THEN
    CREATE ROLE coa_registry LOGIN PASSWORD '$PW' NOSUPERUSER NOCREATEDB NOCREATEROLE CONNECTION LIMIT 20;
  ELSE
    ALTER ROLE coa_registry PASSWORD '$PW';
  END IF;
END \$\$;
SELECT 'CREATE DATABASE coa_registry OWNER coa_registry' WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'coa_registry')\gexec
REVOKE ALL ON DATABASE coa_registry FROM PUBLIC;
GRANT CONNECT ON DATABASE coa_registry TO coa_registry;
SQL
echo "registry role and database are in place"
