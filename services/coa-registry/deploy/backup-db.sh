#!/bin/sh
# /opt/coa/registry/backup-db.sh
# Production logical database backup with retention and atomic completion marker.
# Run on the node: sudo sh backup-db.sh
set -eu

BACKUP_DIR="${BACKUP_DIR:-/opt/coa/backups}"
RETENTION_DAYS="${RETENTION_DAYS:-14}"
TIMESTAMP=$(date -u +"%Y%m%d_%H%M%SZ")
TARGET_FILE="${BACKUP_DIR}/coa_registry_${TIMESTAMP}.dump"
TMP_FILE="${TARGET_FILE}.tmp"
MARKER_FILE="${TARGET_FILE}.complete"

mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR"

echo "[INFO] Starting database backup for coa_registry to ${TARGET_FILE}..."

# Execute pg_dump inside postgres container into temp file
docker exec -i coa-postgres-postgres-1 sh -c 'PGPASSWORD=$(cat /run/secrets/postgres_password) exec pg_dump -U coa -d coa_registry --format=custom --blobs' > "$TMP_FILE"

# Verify dump file is non-empty
FILE_SIZE=$(wc -c < "$TMP_FILE" || stat -c%s "$TMP_FILE" 2>/dev/null || echo 0)
if [ "$FILE_SIZE" -lt 1024 ]; then
    echo "[ERROR] Backup failed or file too small (${FILE_SIZE} bytes)" >&2
    rm -f "$TMP_FILE"
    exit 1
fi

# Atomic commit
mv "$TMP_FILE" "$TARGET_FILE"
sha256sum "$TARGET_FILE" > "${TARGET_FILE}.sha256"
touch "$MARKER_FILE"

echo "[INFO] Backup completed successfully: ${TARGET_FILE} (${FILE_SIZE} bytes)"

# Retention pruning
echo "[INFO] Pruning backups older than ${RETENTION_DAYS} days..."
find "$BACKUP_DIR" -name "coa_registry_*.dump" -mtime +"$RETENTION_DAYS" -exec rm -f {} {}.sha256 {}.complete \; 2>/dev/null || true
echo "[INFO] Backup rotation complete."
