# Registry and Coordinator deployment (Phases 10, 10.1, 11, 12, TLS Gate)

How the Registry and the Coordinator run on `coa-infra-01`, how to rebuild or restart them, and what the gates checked. Protocols: [REGISTRY_PROTOCOL.md](REGISTRY_PROTOCOL.md), [CONTROL_PROTOCOL.md](CONTROL_PROTOCOL.md).
Node facts: [INFRASTRUCTURE.md](INFRASTRUCTURE.md), [VPS_BASELINE.md](VPS_BASELINE.md). `<VPS_IP>` is the node's public IPv4 (kept out of git); no secret is in this file. Production hostname: `coa-manager.duckdns.org`.

## Topology

```
Internet
  |  22/tcp (sshd)   80/tcp (redirect to 443), 443/tcp (Caddy, the only container with published ports)
  v
Caddy  :443  /registry/*  ->  coa-registry:8080      network coa-ingress   (HTTPS; ACME Let's Encrypt)
       :443  /coord/*     ->  coa-coordinator:8081   network coa-ingress   (WSS WebSocket; Phase 12)
                                   |
Registry container  (coa-registry)  +--- network coa-db (internal) ---+
                                                                       v
                                                      PostgreSQL container (alias coa-postgres:5432)
```

* Registry and PostgreSQL publish **no host port**. `docker ps` shows `8080/tcp` and `5432/tcp` (exposed inside Docker networks only), and `ss -tulnH` lists only 22, 80, 443.
  Never add `5432:5432` or `8080:8080`.
* The Registry joins `coa-ingress` (to be reached by Caddy) and `coa-db` (to reach PostgreSQL). PostgreSQL joins only `coa-db`, which is `--internal`.
* Caddy serves public HTTPS and WSS on `coa-manager.duckdns.org` with automatic Let's Encrypt ACME certificate management. Port 80 automatically redirects (308 Permanent Redirect) to HTTPS. Certificate state and ACME account metadata are stored in persistent Docker volumes `coa-caddy_caddy_data` and `coa-caddy_caddy_config`.

## Files (in git: `services/coa-registry/deploy/`)

| In git | On the node |
|---|---|
| `postgres.compose.yaml` | `/opt/coa/postgres/compose.yaml` |
| `registry.compose.yaml` | `/opt/coa/registry/compose.yaml` (+ `.env`: `REGISTRY_TAG`, rate limits) |
| `Caddyfile` | `/opt/coa/caddy/Caddyfile` (the Phase -1 file is kept as `Caddyfile.phase-1.bak`, the Phase-11 one as `.phase-12.bak`) |
| `../../coa-coordinator/deploy/coordinator.compose.yaml` | `/opt/coa/coordinator/compose.yaml` (+ `.env`: `COORDINATOR_TAG`) |
| `../../coa-coordinator/deploy/provision-db.sh` | creates `coa_coordinator`, a role that may read four columns of `realms` (`realm_id`, `public_key`, `published`, `advert_version`) and nothing else; also creates its password file |
| `../../coa-coordinator/deploy/deploy.sh` | builds the Coordinator image on the node and starts it |
| `../../coa-coordinator/Dockerfile` | same shape as the Registry's (non-root uid 10002, read-only root, all capabilities dropped, no-new-privileges, 256 MB, 128 pids) |
| `provision-db.sh` | run once (idempotent) as root: creates the least-privilege role `coa_registry` and database `coa_registry` |
| `deploy.sh` | builds the image on the node from the working tree and restarts the stack |
| `audit-db.sql` / `audit-db.sh` | what the database holds (gate item 17) |
| `gate-restarts.sh` | gate items 2 and 14 |
| `../Dockerfile` | multi-stage build (`rust:1-bookworm` → `debian:bookworm-slim`, non-root uid 10001, read-only root, all capabilities dropped, no-new-privileges, 256 MB, 128 pids) |

## Secrets (never in git)

```
/opt/coa/secrets/postgres_password       root:root   0600   superuser of PostgreSQL; read only by the PostgreSQL entrypoint
/opt/coa/secrets/registry_db_password    root:10001  0440   password of the role coa_registry; mounted at /run/secrets in the Registry container
/opt/coa/secrets/coordinator_db_password root:10002  0440   password of the role coa_coordinator (read-only, four columns); mounted in the Coordinator container
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
curl -s https://coa-manager.duckdns.org/registry/v2/healthz                                 # {"status":"ok","protocol_version":2}
curl -s https://coa-manager.duckdns.org/coord/v1/health                                     # {"hosts":N,"protocol_version":1,"status":"ok"}
sudo sh /opt/coa/registry/src/services/coa-registry/deploy/audit-db.sh                   # contents audit
```

Logs are structured JSON: `realm`, `endpoint`, `status`, `error` (code), `latency_ms`, and the first 8 characters of the capabilities hash. They never contain keys,
signatures, headers, request bodies or the database credentials (checked: 0 occurrences of either password in any container's log). Backups: automated via `backup-db.sh` into `/opt/coa/backups` with SHA-256 verification and atomic markers (verified in Gate 15.5; see INFRASTRUCTURE.md); the data is discovery metadata that every Host re-announces by itself within a heartbeat or two.

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

Abuse controls on the node with the production defaults: 13 registrations from one address gave 10 × 200 and then 429 (`gate_registration_rate_limit`); per-realm heartbeat limits and forged-request costs are covered by `tests/api.rs`.
The gate's test realms were deleted from the database afterwards (`DELETE FROM realms`; the table is empty until a real Host publishes).

**Item 13 in detail.** The real Manager (debug build, merged branch) published the realm *PT Guest* of the owner's smoke setup (cap 60), a character was in a portable session on it (a bot standing in for the client),
and the Registry was stopped for two minutes, then the Manager was pointed at an unroutable address (timeouts), then the Registry was restored without touching the Manager. Throughout: the worldserver answered RA
(`server info`) every poll, the portable session stayed `playing` with one open session, canonical revisions kept advancing from the game (5 → 8, including a deliberate change), the portable runtime's tick
kept running with no error, and the Manager showed *retrying* with a growing delay (5 s doubling, capped), then went back to *online* by itself when the Registry returned (at its next scheduled attempt, within half a minute).
After the logout the final sync was made and the next session armed as usual.

**Found by running it for real** (and fixed): on Windows the key file's access list had to include delete rights or the final rename was refused (`icacls … (F)`); the realm probe, which takes many seconds, was blocking the publishing loop (now a cached, background probe);
a republish within the same second as the last request was refused as a replay (the counter is carried over).

## The Phase-10.1 gate (protocol 2)

Run on 2026-10-07 against the same node. Tools: `services/coa-registry-e2e/tests/live_vps.rs` (`gate_1_to_12_against_the_node` re-run for protocol 2, `gate_v1_record_republishes_under_v2`), `deploy/gate-restarts.sh`, `deploy/audit-db.sh`, and the real Manager window.

| Item | Result |
|---|---|
| A realm republishes under protocol 2 | pass: the real *PT Guest* realm of the smoke setup registered with its structured listing (rates `Rate.*` read from its `worldserver.conf`, modules from the catalog, level cap 60 from the running core, population from its database: 0 players + 12 bots counted separately, capacity none because `PlayerLimit = 0`) |
| A record written under protocol 1 is migrated, not lost | pass (`gate_v1_record_republishes_under_v2`): the Phase-10 probe realm kept its `RealmId`, key and `created_at`; it was invisible to the public list until it registered again under protocol 2 with the same key, which was one metadata revision (1 → 2) |
| Records survive restarts | pass (`gate-restarts.sh` re-run for protocol 2: Registry restart, PostgreSQL restart, both stopped and started in the wrong order, `compose down/up`: after each, the realm and its key are intact; a Registry restart while Players were joining did not disturb them) |
| Protocol 1 is refused by one documented policy | pass: `/registry/v1/*` → `400 unsupported_protocol_version` naming protocol 2 (REGISTRY_PROTOCOL.md section 0); `X-Coa-Registry-Version: 1` on a v2 path → the same code |
| Human and bot counts are separate | pass (two columns, two fields; the list's "N players + M bots") |
| Capabilities are hash-checked; so is the listing | pass: a hash that is not the hash of the content → `400 invalid_metadata`; a heartbeat with another hash and no part → `resend_*` and nothing applied |
| The Phase-10 security gates still pass | pass: `gate_1_to_12_against_the_node` re-run today, 12 items, 45 s |

## The Phase-11 gate (public list)

Run today with 5000 synthetic realms loaded by `deploy/synthetic.sh load 5000` (marked, removed afterwards; the table is back to the one real realm), through the public path (workstation → Internet → Caddy → Registry → PostgreSQL):

| Item | Result |
|---|---|
| Paging through thousands of realms | pass: for each of 4 sorts (players, name, cap, created) 5001 realms in 51 pages, no repeat, none skipped, in the order the sort promises (`gate11_the_public_list_pages_through_thousands_of_realms`); 204 page requests, median 107 ms, p95 112 ms, worst 205 ms (the time is the round trip from here, not the query) |
| The query plan stays an index scan | pass (`synthetic.sh explain`): `realms_browse_name`, `_players`, `_cap`, `_created`, 0.25–0.76 ms for a page of 51 with 5000 rows |
| Filters, bounds, ETag/cache, read-only | pass (`gate11_filters_etag_and_read_only`): search, mode, cap range, module, online players; limits above 100, a repeated or unknown parameter and a foreign cursor → 400; `If-None-Match` → 304; `POST`/`PUT`/`DELETE` on the public paths → 405 |
| The Manager's *Servers* page | pass in the real window against the live Registry: the table, filters, sort headers, module chips and tooltips from the local catalog (an unknown id as plain text), the detail drawer with the compatibility verdict of the selected portable character, "—" for ping |
| Configurable Registry address, none hard-coded | pass (setting + `COA_REGISTRY_URL`) |
| **TLS** | **pass**: public domain `coa-manager.duckdns.org` configured; Caddy automatically obtains and renews trusted public Let's Encrypt certificates; HTTP redirects to HTTPS; certificate state persisted across restarts |

## The TLS Finishing Gate (Phases 11 and 12)

Run from an external workstation against `https://coa-manager.duckdns.org` and `wss://coa-manager.duckdns.org`.

| # | Item | Result |
|---|---|---|
| 1 | `https://coa-manager.duckdns.org/registry/v2/healthz` returns Registry v2 health | pass (`{"status":"ok","protocol_version":2}`) |
| 2 | HTTP request redirects to HTTPS | pass (`HTTP/1.1 308 Permanent Redirect` -> `https://coa-manager.duckdns.org/...`) |
| 3 | Public realm browser works through HTTPS | pass (`gate11_filters_etag_and_read_only` over HTTPS) |
| 4 | Host publication / register / heartbeat works through HTTPS | pass (`gate_1_to_12_against_the_node` over HTTPS) |
| 5 | Coordinator Host connection works through WSS | pass (`HostLink` connected to `wss://coa-manager.duckdns.org/coord/v1/host`) |
| 6 | Player → Coordinator → Host control channel works through WSS | pass (`PlayerControl::connect` establishes Noise session over WSS) |
| 7 | Automatic account provisioning works | pass (`ensure_account` provisioned account over WSS) |
| 8 | Existing-account linking works | pass (`link_existing` linked credentials over WSS) |
| 9 | Character claim works | pass (`claim` and `acknowledge` over WSS) |
| 10 | Caddy restart recovers without broken/new certificate | pass (`docker restart` serves cached cert from `/data/caddy/certificates`) |
| 11 | Certificate inspected: hostname, chain, dates | pass (`CN=coa-manager.duckdns.org`, Issuer: Let's Encrypt, Valid until Jan 2027, Verify=True) |
| 12 | External port scan exposes no internal ports | pass (22, 80, 443 open; 2019, 5432, 8080, 8081 closed) |
| 13 | Registry/Coordinator/control-plane regression tests pass | pass (all suites pass cleanly) |

## The Coordinator (Phase 12)

`services/coa-coordinator` (own Cargo workspace, like the Registry; `crates/coa-control-proto` is the shared protocol). One container, no published port, Caddy proxies `/coord/*`
(the health check is `/coord/v1/health`). It holds **nothing on disk and writes nothing to any database**: the connected Hosts are an in-memory table; it reads the realm's public key
from the Registry's database as `coa_coordinator` (`SELECT (realm_id, public_key, published, advert_version) ON realms`, `default_transaction_read_only`, 4 connections), only to check a Host's proof.

Limits (defaults of `coa_control_proto::coord::limits`, enforced by the service and tested): 16 simultaneous Player channels per Host, 8 connections per source address, 60 new Player channels per minute per realm and 30 per
address, 16 MiB through one channel, a channel lives at most 600 s and at most 60 s silent, the Host has 10 s to answer a new channel, 20 KiB per forwarded frame, 10 s to say hello, a hello's timestamp within 120 s.

Operating: `VPS=ubuntu@<VPS_IP> TAG=<tag> sh services/coa-coordinator/deploy/deploy.sh` (provisions the role, builds on the node, starts); `sudo docker logs coa-coordinator-coordinator-1` (JSON lines: realm id, channel number, player id, address, byte count, seconds — never a name, a password or a payload; the gate greps for them).
A restart of the Coordinator drops the Host links; the Hosts reconnect on their own (checked: 5 s, backoff 2 s → 60 s) and joining works again at once. A restart of the Registry changes nothing for established links.
The Phase-12 gate results are in [CONTROL_PROTOCOL.md](CONTROL_PROTOCOL.md) section 11.
