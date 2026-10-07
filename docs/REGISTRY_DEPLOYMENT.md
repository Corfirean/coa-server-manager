# Registry deployment (Phase 10)

How the Registry runs on `coa-infra-01`, how to rebuild or restart it, and what the Phase-10 gate checked. Protocol: [REGISTRY_PROTOCOL.md](REGISTRY_PROTOCOL.md).
Node facts: [INFRASTRUCTURE.md](INFRASTRUCTURE.md), [VPS_BASELINE.md](VPS_BASELINE.md). `<VPS_IP>` is the node's public IPv4 (kept out of git); no secret is in this file.

## Topology

```
Internet
  |  22/tcp (sshd)   80/tcp, 443/tcp (Caddy, the only container with published ports)
  v
Caddy  :80   /registry/*  ->  coa-registry:8080      network coa-ingress
                                   |
Registry container  (coa-registry)  +--- network coa-db (internal) ---+
                                                                       v
                                                      PostgreSQL container (alias coa-postgres:5432)
```

* Registry and PostgreSQL publish **no host port**. `docker ps` shows `8080/tcp` and `5432/tcp` (exposed inside Docker networks only), and `ss -tulnH` lists only 22, 80, 443.
  Never add `5432:5432` or `8080:8080`.
* The Registry joins `coa-ingress` (to be reached by Caddy) and `coa-db` (to reach PostgreSQL). PostgreSQL joins only `coa-db`, which is `--internal`.
* Caddy serves plain HTTP on :80 (no domain yet: `auto_https off`, admin API off). Port 443 is allowed by UFW and published by Docker, but nothing listens on it until TLS exists.
  **HTTPS is mandatory before Phase 11 is treated as release-ready**; for this staging gate HTTP is acceptable because every Host request is signed and carries no secret.

## Files (in git: `services/coa-registry/deploy/`)

| In git | On the node |
|---|---|
| `postgres.compose.yaml` | `/opt/coa/postgres/compose.yaml` |
| `registry.compose.yaml` | `/opt/coa/registry/compose.yaml` (+ `.env`: `REGISTRY_TAG`, rate limits) |
| `Caddyfile` | `/opt/coa/caddy/Caddyfile` (the Phase -1 file is kept as `Caddyfile.phase-1.bak`) |
| `provision-db.sh` | run once (idempotent) as root: creates the least-privilege role `coa_registry` and database `coa_registry` |
| `deploy.sh` | builds the image on the node from the working tree and restarts the stack |
| `audit-db.sql` / `audit-db.sh` | what the database holds (gate item 17) |
| `gate-restarts.sh` | gate items 2 and 14 |
| `../Dockerfile` | multi-stage build (`rust:1-bookworm` → `debian:bookworm-slim`, non-root uid 10001, read-only root, all capabilities dropped, no-new-privileges, 256 MB, 128 pids) |

## Secrets (never in git)

```
/opt/coa/secrets/postgres_password       root:root   0600   superuser of PostgreSQL; read only by the PostgreSQL entrypoint
/opt/coa/secrets/registry_db_password    root:10001  0440   password of the role coa_registry; mounted at /run/secrets in the Registry container
```

Both were generated on the node (`/dev/urandom`, 40 characters) and never displayed. The Registry connects as `coa_registry` (not a superuser, no CREATEDB/CREATEROLE,
connection limit 20, owner of its own database only); PUBLIC has no access to that database. Passwords reach `psql` through the container's own secret or stdin, never a command line.
Rotating: write a new value to the file, `sudo sh provision-db.sh` (it `ALTER`s the role), `docker compose up -d --force-recreate` in `/opt/coa/registry`.

## First deployment (what was done)

```bash
# 1. secrets (on the node)
sudo sh -c 'umask 077; head -c 48 /dev/urandom | base64 | tr -dc A-Za-z0-9 | head -c 40 > /opt/coa/secrets/postgres_password'
sudo sh -c 'umask 077; head -c 48 /dev/urandom | base64 | tr -dc A-Za-z0-9 | head -c 40 > /opt/coa/secrets/registry_db_password'
sudo chown root:10001 /opt/coa/secrets/registry_db_password && sudo chmod 440 /opt/coa/secrets/registry_db_password
# 2. PostgreSQL (started now, for the Registry only) and the Registry's role/database
cd /opt/coa/postgres && sudo docker compose up -d && sudo sh provision-db.sh
# 3. from a workstation, at the repository root: build on the node and start the Registry
VPS=ubuntu@<VPS_IP> TAG=p10 sh services/coa-registry/deploy/deploy.sh
# 4. Caddy: copy the Caddyfile, validate, restart
sudo docker compose -f /opt/coa/caddy/compose.yaml exec caddy caddy validate --config /etc/caddy/Caddyfile
sudo docker compose -f /opt/coa/caddy/compose.yaml restart caddy
```

Migrations run at start-up under an advisory lock; the Registry waits for a database that is still starting and refuses one that is ahead of its binary.
Staging note: the gate ran with `REGISTRY_REGISTER_BURST=100`; the production defaults (burst 10, 10 per hour) are what `registry.compose.yaml` uses when `.env` does not override them.

## Operating

```bash
cd /opt/coa/registry && sudo docker compose ps && sudo docker compose logs --tail 50     # JSON lines
curl -s http://<VPS_IP>/registry/v1/healthz                                              # {"status":"ok","protocol_version":1}
sudo sh /opt/coa/registry/src/services/coa-registry/deploy/audit-db.sh                   # contents audit
```

Logs are structured JSON: `realm`, `endpoint`, `status`, `error` (code), `latency_ms`, and the first 8 characters of the capabilities hash. They never contain keys,
signatures, headers, request bodies or the database credentials (checked: 0 occurrences of either password in any container's log). Backups: not yet automated
(`pg_dump coa_registry` to `/opt/coa/backups`, see INFRASTRUCTURE.md); the data is discovery metadata that every Host re-announces by itself within a heartbeat or two.

## The Phase-10 gate

Run on 2026-10-07 against `http://<VPS_IP>` (Windows workstation → Internet → Caddy → Registry → PostgreSQL). Tools: `services/coa-registry-e2e/tests/live_vps.rs`
(`REGISTRY_URL=http://<VPS_IP> cargo test --test live_vps -- --ignored --nocapture`), `deploy/gate-restarts.sh`, `deploy/audit-db.sh`, a TCP connect scan of 1–65535, and the real Manager window.

| # | Item | Result |
|---|---|---|
| 1 | Fresh realm generates identity and registers | pass (`RegistryHost` made the UUIDv7 and the key, registered; the record shows `published`, `online`, revision 1) |
| 2 | Registry restart keeps the realm | pass (`docker restart`; the realm is still registered and heartbeats with its own key) |
| 3 | Host Manager restart keeps RealmId and key | pass (same id and public key, registered again as itself, revision unchanged) |
| 4 | Valid heartbeat updates last_seen | pass |
| 5 | Missing heartbeats → offline after the TTL, not deleted | pass (125 s without a heartbeat: `online=false`, `published=true`; a heartbeat brings it back) |
| 6 | Metadata/capability change updates the record | pass (cap 60 → 70: revision 1 → 2, new capabilities stored) |
| 7 | Same RealmId, another key | pass (`register` 403 `realm_key_mismatch`; heartbeat/unpublish/read 401 `invalid_signature`; the record is untouched) |
| 8 | Tampered body | pass (401 `invalid_signature`) |
| 9 | Old/future timestamp | pass (±1 h: 401 `bad_timestamp`; a non-increasing one: 401 `timestamp_not_monotonic`) |
| 10 | Unsupported protocol version | pass (header and body: 400 `unsupported_protocol_version`) |
| 11 | Oversized/hostile metadata | pass (413; 500-character name, 5000-byte description, `<script>` language, direction override, 6 unknown fields (`character`, `snapshot`, `password`, `ra_password`, `db_credentials`, `canonical_revision`), 30000-deep JSON: 400; nothing stored) |
| 12 | Unpublish stops publication | pass (`published=false`, `online=false`; heartbeat 409 `not_published`; registering again republishes the same id) |
| 13 | A Registry/network outage does not disturb the worldserver or portable sessions | pass (below) |
| 14 | Registry and PostgreSQL container restarts recover | pass (`gate-restarts.sh`: Registry restart, PostgreSQL restart alone, both stopped and started in the wrong order, `compose down/up` of both; after each the realm and its key are intact) |
| 15 | External scan shows only 22, 80, 443 | pass (all 65535 TCP ports: 22 and 80 answer; 443 is allowed and published but refuses until TLS exists; nothing else) |
| 16 | PostgreSQL not reachable externally | pass (5432 closed from outside; the container has no published port; `ss` on the node shows no 5432 or 8080 listener) |
| 17 | No credentials, snapshots or character records in the database | pass (two tables, `realms` and `schema_migrations`; no column or table named for players, characters, accounts, sessions, snapshots, credentials; 0 rows mention any forbidden word; the largest capabilities blob is under 1 KB) |

**Item 13 in detail.** The real Manager (debug build, merged branch) published the realm *PT Guest* of the owner's smoke setup (cap 60), a character was in a portable session on it (a bot standing in for the client),
and the Registry was stopped for two minutes, then the Manager was pointed at an unroutable address (timeouts), then the Registry was restored without touching the Manager. Throughout: the worldserver answered RA
(`server info`) every poll, the portable session stayed `playing` with one open session, canonical revisions kept advancing from the game (5 → 8, including a deliberate change), the portable runtime's tick
kept running with no error, and the Manager showed *retrying* with a growing delay (5 s doubling, capped), then went back to *online* by itself when the Registry returned (at its next scheduled attempt, within half a minute).
After the logout the final sync was made and the next session armed as usual.

**Found by running it for real** (and fixed): on Windows the key file's access list had to include delete rights or the final rename was refused (`icacls … (F)`); the realm probe, which takes many seconds, was blocking the publishing loop (now a cached, background probe);
a republish within the same second as the last request was refused as a replay (the counter is carried over).
