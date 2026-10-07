#!/bin/sh
# Phase-10 gate items 2 and 14, driven from a workstation:  VPS=ubuntu@<VPS_IP> REGISTRY_URL=http://<VPS_IP> sh gate-restarts.sh
# A realm is registered once (random key); after every restart below it must still be there and must still be able to heartbeat with its own key.
set -u
: "${VPS:?}" "${REGISTRY_URL:?}"
E2E="$(cd "$(dirname "$0")/../../coa-registry-e2e" && pwd)"
run() { (cd "$E2E" && REGISTRY_URL="$REGISTRY_URL" "$@" 2>&1); }
remote() { ssh "$VPS" "$1"; }
wait_healthy() { # $1 container, $2 seconds
  i=0; while [ $i -lt "$2" ]; do
    s=$(remote "sudo docker inspect -f '{{.State.Health.Status}}' $1 2>/dev/null"); [ "$s" = healthy ] && return 0; i=$((i+2)); sleep 2; done; return 1; }
wait_api() { i=0; while [ $i -lt 90 ]; do code=$(curl -s -o /dev/null -m 5 -w '%{http_code}' "$REGISTRY_URL/registry/v2/healthz"); [ "$code" = 200 ] && return 0; i=$((i+2)); sleep 2; done; return 1; }

out=$(run cargo test --test live_vps gate_persist_probe_write -- --ignored --nocapture | grep PERSIST_REALM)
set -- $out; REALM=$2; SEED=$3
[ -n "$REALM" ] || { echo "GATE FAIL: probe write"; exit 1; }
echo "probe realm $REALM written"
check() { # $1 label
  r=$(PERSIST_REALM=$REALM PERSIST_SEED=$SEED run cargo test --test live_vps gate_persist_probe_read -- --ignored --nocapture | grep -E "PERSIST_OK|panicked")
  case "$r" in *PERSIST_OK*) echo "GATE PASS  $1: realm still registered, own key still accepted";; *) echo "GATE FAIL  $1: $r"; return 1;; esac; }

echo "--- 2. Registry container restart"
remote "sudo docker restart coa-registry-registry-1 >/dev/null"; wait_healthy coa-registry-registry-1 90 && wait_api && check "2: after restarting the Registry container"

echo "--- 14a. PostgreSQL container restart (health is degraded meanwhile, then recovers by itself)"
remote "sudo docker restart coa-postgres-postgres-1 >/dev/null"
sleep 1; echo "health during the restart: $(curl -s -m 5 -o /dev/null -w '%{http_code}' "$REGISTRY_URL/registry/v2/healthz")"
wait_healthy coa-postgres-postgres-1 90 && wait_api && check "14a: after restarting PostgreSQL (the Registry container was not restarted)"

echo "--- 14b. both stopped, PostgreSQL started later than the Registry (the Registry waits for it)"
remote "sudo docker stop coa-registry-registry-1 coa-postgres-postgres-1 >/dev/null; sudo docker start coa-registry-registry-1 >/dev/null"
sleep 6; echo "Registry up without a database: health $(curl -s -m 5 -o /dev/null -w '%{http_code}' "$REGISTRY_URL/registry/v2/healthz")"
remote "sudo docker start coa-postgres-postgres-1 >/dev/null"
wait_healthy coa-postgres-postgres-1 90 && wait_healthy coa-registry-registry-1 120 && wait_api && check "14b: after a cold start in the wrong order"

echo "--- 14c. the stack taken down and brought up again (the data lives in the volume)"
remote "cd /opt/coa/registry && sudo docker compose down >/dev/null 2>&1; cd /opt/coa/postgres && sudo docker compose down >/dev/null 2>&1; cd /opt/coa/postgres && sudo docker compose up -d >/dev/null 2>&1; cd /opt/coa/registry && sudo docker compose up -d >/dev/null 2>&1"
wait_healthy coa-postgres-postgres-1 90 && wait_healthy coa-registry-registry-1 120 && wait_api && check "14c: after compose down/up of both stacks"
remote "sudo docker ps --format '{{.Names}} {{.Status}}'"
