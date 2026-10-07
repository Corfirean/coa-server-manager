#!/bin/sh
# Create the Coordinator's database role (idempotent). Run on the node as root: sudo sh provision-db.sh
# The role may read exactly four columns of one table. The password is read from /opt/coa/secrets/coordinator_db_password (created here when missing) and goes to psql on stdin.
set -eu
umask 077
if [ ! -s /opt/coa/secrets/coordinator_db_password ]; then
  head -c 32 /dev/urandom | base64 | tr -d '/+=\n' | head -c 40 > /opt/coa/secrets/coordinator_db_password
fi
chown root:10002 /opt/coa/secrets/coordinator_db_password
chmod 0440 /opt/coa/secrets/coordinator_db_password
PW=$(cat /opt/coa/secrets/coordinator_db_password)
docker exec -i coa-postgres-postgres-1 sh -c 'PGPASSWORD=$(cat /run/secrets/postgres_password) exec psql -U coa -d coa_registry -v ON_ERROR_STOP=1 -q' <<SQL
DO \$\$ BEGIN
  IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'coa_coordinator') THEN
    CREATE ROLE coa_coordinator LOGIN PASSWORD '$PW' NOSUPERUSER NOCREATEDB NOCREATEROLE CONNECTION LIMIT 8;
  ELSE
    ALTER ROLE coa_coordinator PASSWORD '$PW';
  END IF;
END \$\$;
REVOKE ALL ON DATABASE coa_registry FROM coa_coordinator;
GRANT CONNECT ON DATABASE coa_registry TO coa_coordinator;
REVOKE ALL ON ALL TABLES IN SCHEMA public FROM coa_coordinator;
GRANT USAGE ON SCHEMA public TO coa_coordinator;
GRANT SELECT (realm_id, public_key, published, advert_version) ON realms TO coa_coordinator;
SQL
echo "coordinator role is in place"
