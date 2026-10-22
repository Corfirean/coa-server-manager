#!/bin/sh
# /opt/coa/registry/restore-db.sh
# Production database restore from an existing backup dump file.
# Usage: sudo sh restore-db.sh /opt/coa/backups/coa_registry_YYYYMMDD_HHMMSSZ.dump
set -eu

if [ $# -lt 1 ]; then
    echo "Usage: $0 <path_to_backup_dump>" >&2
    exit 1
fi

BACKUP_FILE="$1"

if [ ! -f "$BACKUP_FILE" ]; then
    echo "[ERROR] Backup file not found: $BACKUP_FILE" >&2
    exit 1
fi

# Verify checksum if present
if [ -f "${BACKUP_FILE}.sha256" ]; then
    echo "[INFO] Verifying backup checksum..."
    sha256sum -c "${BACKUP_FILE}.sha256"
fi

echo "[WARNING] Restoring database coa_registry from $BACKUP_FILE. Existing connections will be terminated."
read -p "Type 'RESTORE' to proceed: " CONFIRM
if [ "$CONFIRM" != "RESTORE" ]; then
    echo "[INFO] Restore aborted."
    exit 0
fi

# Terminate existing connections to coa_registry
docker exec -i coa-postgres-postgres-1 sh -c 'PGPASSWORD=$(cat /run/secrets/postgres_password) exec psql -U coa -d coa -v ON_ERROR_STOP=1 -q' <<SQL
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = 'coa_registry' AND pid <> pg_backend_pid();
SQL

# Restore dump
docker exec -i coa-postgres-postgres-1 sh -c 'PGPASSWORD=$(cat /run/secrets/postgres_password) exec pg_restore -U coa -d coa_registry --clean --if-exists --no-owner' < "$BACKUP_FILE"

echo "[INFO] Database restore completed successfully from $BACKUP_FILE."
