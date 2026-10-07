# Registry protocol 1 (Phase 10)

The central realm Registry is **discovery and presence infrastructure only**. It answers two questions: *which realms exist and who publishes them*, and
*are they alive*. Code: `crates/coa-registry-proto` (the protocol, shared by both sides), `services/coa-registry` (the service),
`crates/coa-core/src/realm_registry` (the Host Manager's client and publishing loop).

`REGISTRY_PROTOCOL_VERSION = 1`. It is the Registry's own number; it has nothing to do with the portable character, session or capability-profile versions.

## 1. Trust model

The Registry never receives or stores: MySQL credentials, RA credentials, game account passwords, `PortableCharacter` snapshots, canonical revisions or
payloads, inventories, portable session payloads, player character ids. There is no field for any of them; every request type refuses unknown fields,
so nothing of the sort can be smuggled in, and the database has no table or column that could hold it (`deploy/audit-db.sql`).

Registering is not trust. The capabilities a Registry publishes are **host-advertised discovery metadata**: a later JOIN negotiates capabilities again with the
live Host. The Registry checks that an advertisement is well-formed, bounded and self-consistent (its hash is the hash of its content), not that it is true.

There is no shared admin secret, no bearer token, no unauthenticated admin API, and no public realm list yet.

## 2. Identity

Every publishable realm has an identity independent of its address: `RealmId` (a UUIDv7, lower-case hyphenated; any other spelling is refused so that one id
has one spelling in the signed input) and an Ed25519 key pair, **one per realm** (not per Manager installation). The first registration carries the public key
and proves possession by signing the request. Afterwards the Registry holds the key for that id: a request for the id signed by any other key is refused
(`401 invalid_signature`), and a registration for the id with another key is refused (`403 realm_key_mismatch`). Nobody can take an id over.

The private key stays in the Host Manager's storage (section 8) and never enters a log, a diagnostic, a descriptor or the Registry.

## 3. Signed requests

Every mutating request (and the authenticated read) is signed.

Headers:

| Header | Value |
|---|---|
| `X-Coa-Registry-Version` | `1` |
| `X-Coa-Realm` | the realm id; for every endpoint except `register` it must equal the id in the path |
| `X-Coa-Timestamp` | unix seconds (decimal) |
| `X-Coa-Signature` | base64url (no padding, 86 characters) of the 64-byte Ed25519 signature |

The signed input is these seven lines, joined by a single `\n` (no trailing newline), as UTF-8 bytes:

```
coa-registry-sig-v1
<protocol version, decimal>
<HTTP method, upper case>
<request path exactly as sent; no query string is allowed>
<realm id>
<unix timestamp, decimal>
<SHA-256 of the body bytes as sent, lower-case hex>      (the empty body for a GET)
```

The signature is made over those bytes with the realm's Ed25519 key (RFC 8032, deterministic). Verification is strict (`verify_strict`).

### Rules checked, in this order

1. `429 rate_limited`: the source address is over its allowance (section 6).
2. `413 request_too_large`: more than 65536 body bytes.
3. `400 malformed_request`: a header is missing or malformed; the path has a query string; the body is not the expected JSON, or has an unknown field.
4. `400 unsupported_protocol_version`: the header, or `protocol_version` in the body, is not `1`.
5. `400 invalid_realm_id`: not a canonical UUIDv7, or the header and the path (or body) name different realms.
6. `401 bad_timestamp`: the timestamp is more than 300 s from the Registry's clock (either way).
7. For `register`: `400 invalid_metadata` (a limit or consistency rule below), then the signature is verified with the key *in the body*.
   For the others: `404 unknown_realm`, then the signature is verified with the *stored* key.
8. `401 invalid_signature`: the signature does not match (a forged request also costs its sender more of its address allowance).
9. `403 realm_key_mismatch`: `register` for a known id with another key (cannot happen with a valid signature from the stored key).
10. `401 timestamp_not_monotonic`: the timestamp is not **strictly later than the last request applied for this realm**. This is the replay protection: a captured
    request cannot be sent again, and one delayed past a later request is refused. The Host never reuses a second: it signs `max(clock, last + 1)`.
11. `429 rate_limited` per realm (writes only): 4 immediately, then one every 10 s.

### Test vectors

Key: the Ed25519 seed is 32 bytes of `0x07`; public key (base64url) `6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw`.
Realm `018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d`.

Heartbeat, body `{"protocol_version":1}` (SHA-256 `00aa4c2c857995eb8e19cb0fade07e4b49aac774e37a99c37a0c9f549204d9de`), timestamp `1790000000`:

```
coa-registry-sig-v1
1
POST
/registry/v1/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d/heartbeat
018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d
1790000000
00aa4c2c857995eb8e19cb0fade07e4b49aac774e37a99c37a0c9f549204d9de
```

signature `1ys2C7RVjhEtTl0bVrdowpuPELwOnhuiYZKp88dAWdcA3cezP9QuI903OJi2N2mFA9kifblD_fvdT2B_8F5_Dw`.

Authenticated read, `GET /registry/v1/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d`, empty body (SHA-256 `e3b0c442…b855`), timestamp `1790000030`:
signature `38nHMS6yvl8yrTbLmG1w5LiLVkvTaiRdQPrgT2GSvtR8jW_W0z50IRxD_kInBYP5Yl1Qwwm-r6BVDdTR5ts-BQ`.

Both are checked by `sign::tests` and independently by `tools/registry-vector-check.py` (RFC 8032 reference algorithm in plain Python; it prints the public key and the heartbeat signature).

## 4. Endpoints

| | |
|---|---|
| `GET  /registry/v1/healthz` | unauthenticated: `{"status":"ok","protocol_version":1}`, `503 degraded` when the database is unreachable |
| `POST /registry/v1/realms/register` | create the realm's record, or announce the same key again; publishes |
| `POST /registry/v1/realms/{realm_id}/heartbeat` | presence, player count, and metadata that changed |
| `POST /registry/v1/realms/{realm_id}/unpublish` | stop publishing (the record stays) |
| `GET  /registry/v1/realms/{realm_id}` | the authenticated self read (integration tests, support); **not** a public list |

Anything else is `404`. There is no realm list, no search and no admin endpoint.

Errors are `{"error":{"code":"<code>","message":"…"}}` with the codes above plus `unknown_realm` (404), `not_published` (409), `unavailable` (503, storage), `internal` (500).
Codes a repeat cannot fix are *permanent* (`malformed_request`, `unsupported_protocol_version`, `request_too_large`, `invalid_metadata`, `invalid_realm_id`, `invalid_signature`,
`realm_key_mismatch`): the Host stops instead of retrying.

### Register

```json
{ "protocol_version": 1, "realm_id": "…", "public_key": "<base64url, 43 chars>",
  "display_name": "…", "description": "…", "language": "en", "ruleset": "coa",
  "manager_version": "0.6.6",
  "capabilities": { …RealmCapabilities profile version 2… },
  "capabilities_hash": "<64 hex>", "player_count": 3, "player_capacity": 100 }
```

Response: `created`, `metadata_revision`, `published`, `capabilities_hash`, `server_time`, `heartbeat_interval_secs` (30), `online_ttl_secs` (120).
Registering a known id with the same key is idempotent and is how a restarted Manager (or a republication after `unpublish`) announces itself.

### Heartbeat

`capabilities_hash` is always sent; `capabilities` only when it differs from the hash the Registry last acknowledged. Optional: `manager_version`, `player_count`,
`player_capacity` (absent = unknown), `display_name`/`description`/`language` (absent = unchanged). If the hash differs and the capabilities were not sent, the
answer carries `resend_capabilities: true` and nothing is applied; the Host sends them with the next heartbeat. An unchanged heartbeat writes only
`last_seen_at`, the player numbers and the replay counter; the capabilities JSON is rewritten only when its hash changed. A heartbeat of an unpublished realm is
`409 not_published`; the Host registers again to publish.

### Metadata revision

`metadata_revision` starts at 1 and grows by one whenever the display name, description, language, ruleset, Manager version, capabilities, or the published state
changes; presence and player counts do not move it. The Registry returns it in every answer so that an update is explicit.

## 5. Limits

| What | Limit |
|---|---|
| request body | 65536 bytes |
| display name | 1..80 characters, no control characters, no direction overrides (U+202A–202E, U+2066–2069, U+200E/F), no surrounding space |
| description | 1024 bytes (UTF-8); control characters other than `\n` refused; plain text, never HTML |
| language | a short tag (`en`, `pt-BR`), ≤ 16 bytes |
| Manager version | ≤ 32 bytes of `[A-Za-z0-9.+_-]` |
| player count / capacity | ≤ 100000, count ≤ capacity |
| capabilities | profile version 2 only; field by field (≤ 64 extensions, ≤ 16 collection kinds, ≤ 8 client tables, tokens ≤ 64 characters, hashes 64 hex), unknown fields refused, `content_profile_hash` must equal the hash of the content, `ruleset` must equal the capabilities' ruleset |
| `capabilities_hash` | must equal `advert_hash`: SHA-256 of `coa-registry-advert-v1\0` ‖ content hash ‖ the progression numbers and signature (so a level-cap change moves it) |

The advertised type mirrors `RealmCapabilities` field for field (same names, same order: the content hash is taken over the JSON); `coa-core` tests that a real
profile parses into it and keeps its hash, so the two cannot drift apart unnoticed. Consumers must render names and descriptions as plain text.

## 6. Presence and abuse controls

* Heartbeat every ≈30 s (±10 % jitter); online = published and heard of within **120 s**. Missing heartbeats make a realm offline; they never delete it.
* A Host sends heartbeats only while its realm is running; a stopped realm goes offline after the TTL.
* Per source address: 120 requests/minute (burst 120); registration: burst 10, 10 per hour (production defaults; the staging gate ran with 100). A forged request costs 5.
  The address is the last entry of `X-Forwarded-For`, which Caddy sets to the client address it saw; the Registry publishes no port, so only Caddy can reach it.
* Per realm: heartbeat and unpublish burst 4, one more every 10 s (applied only after the signature was verified, so forged traffic cannot throttle a real realm).
* 64 requests in flight, 256 open connections, 10 s request timeout, database pool of 8, `statement_timeout` 5 s, `lock_timeout` 3 s, a 70 KB body cap at Caddy as well.

## 7. Storage

PostgreSQL, forward-only migrations (`migrations/NNNN_name.sql`, recorded in `schema_migrations`; a database that is ahead of the binary is refused). One table:
`realms` (realm id, public key, created/updated/last-seen, published, display name, description, language, ruleset, Manager version, capabilities JSONB and hash,
metadata revision, last request timestamp, player count and capacity). There is no heartbeat history and no table about players, characters, accounts or relays.

## 8. The Host side

`crates/coa-core/src/realm_registry`:

* **Identity**: created the first time publishing is enabled for a local realm. The key is behind `KeyStore`; the implementation (`FileKeyStore`) keeps
  `<data>/registry/keys/<realm id>.key` (base64url of the 32-byte seed), created with its permissions restricted *before* the secret is written: mode 0600 on Unix, and
  on Windows the inherited permissions removed and the current user granted full control (`icacls`). **Limitation:** this is a file; it is weaker than DPAPI or the Credential Manager, which
  the project does not use yet, and anybody who can read the user's files can read the key. It is never written to the realm descriptor, `registry.json`, a log, the diagnostics or the status shown to the interface. If the key
  is missing, publication stops with a message; publishing again makes a *new* identity (it never guesses the old key).
* **Settings**: `<data>/registry/registry.json` (the Registry address, and per local realm: enabled, realm id, name, description, language).
* **Lifecycle** (`RegistryHost`, on its own thread, `RegistryRuntime`): publish → ensure identity → register → heartbeat every ~30 s; changed capabilities or metadata ride the next heartbeat;
  a Manager restart resumes with the same id and key (registering again as itself); unpublish is retried in the background until delivered or refused. The loop only *reads* what a realm
  advertises (probing is done on a separate thread and cached), so a Registry that is down, slow or hostile cannot touch the worldserver or any portable session; nothing here writes portable state.
* **Retry**: transient failures (network, timeout, 5xx, rate limit) back off 5 s → 300 s with ±25 % jitter. `unknown_realm`/`not_published` register again. Clock refusals retry with backoff up to 20 times.
  Permanent refusals stop the realm's publication (`rejected`) until the owner presses *Try again* or republishes.
* **Interface**: *Settings → Public listing* of a server (address of the Registry, name, description, language, *Publish*/*Stop publishing*, status).

## 9. Not in this phase

Public realm browser, JOIN, remote account provisioning, portable-character network transport, Relay, direct-connect optimisation, Coordinator, player accounts on the node.
HTTPS is mandatory before Phase 11 is treated as release-ready (the staging node serves plain HTTP: requests are signed and carry no secret).
