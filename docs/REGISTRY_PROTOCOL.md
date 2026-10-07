# Registry protocol 2 (Phase 10.1, public list in Phase 11)

The central realm Registry is **discovery and presence infrastructure only**. It answers: *which realms exist and who publishes them*, *what do they offer* and
*are they alive*. Code: `crates/coa-registry-proto` (the protocol, shared by both sides), `services/coa-registry` (the service),
`crates/coa-core/src/realm_registry` (the Host Manager's client and publishing loop, and the player's read-only browse client).

`REGISTRY_PROTOCOL_VERSION = 2`. It is the Registry's own number; it has nothing to do with the portable character, session or capability-profile versions, nor with
the control protocol of `docs/CONTROL_PROTOCOL.md`.

## 0. Protocol 1 and what happened to it (the one documented policy)

Protocol 1 (Phase 10) carried a flat description. Protocol 2 replaces it with a structured listing. The policy, applied by code and checked by the live gate
(`gate_v1_record_republishes_under_v2`):

* **Protocol 1 requests are refused explicitly**: anything under `/registry/v1/` answers `400 unsupported_protocol_version` ("protocol version 1 is no longer supported; this Registry speaks
  protocol 2 under /registry/v2"). Nothing is accepted silently and nothing is converted on the fly. A Host Manager that speaks protocol 1 is told so and stops publishing.
* **Records are migrated forward only** (`migrations/0002_protocol_v2.sql`): a realm registered under protocol 1 keeps its `RealmId`, its public key and its creation time. It is marked
  `advert_version = 1`, is **not shown in any public list** and is not offered for hosting, until its Host republishes under protocol 2 (a `register` with the same key, which is
  the normal start-up of a Manager); the record is then an `advert_version = 2` record with the same identity.
* There is no downgrade path and no dual-protocol service.

## 1. Trust model

The Registry never receives or stores: MySQL credentials, RA credentials, game account passwords, `PortableCharacter` snapshots, canonical revisions or
payloads, inventories, portable session payloads, player character ids. There is no field for any of them; every request type refuses unknown fields,
so nothing of the sort can be smuggled in, and the database has no table or column that could hold it (`deploy/audit-db.sql`). Phase 12 adds nothing to this: the control plane
(`docs/CONTROL_PROTOCOL.md`) lives in a separate service and reads exactly one column of this database that is not public anyway (the realm's public key).

Registering is not trust. What a Registry publishes is **host-advertised discovery metadata**: a later JOIN negotiates capabilities again with the live Host. The Registry checks that
an advertisement is well-formed, bounded and self-consistent (its hashes are the hashes of its content), not that it is true.

There is no shared admin secret, no bearer token and no unauthenticated admin API. The public read API (section 5) exposes only what a realm chose to publish.

## 2. Identity

Every publishable realm has an identity independent of its address: `RealmId` (a UUIDv7, lower-case hyphenated; any other spelling is refused so that one id
has one spelling in the signed input) and an Ed25519 key pair, **one per realm** (not per Manager installation). The first registration carries the public key
and proves possession by signing the request. Afterwards the Registry holds the key for that id: a request for the id signed by any other key is refused
(`401 invalid_signature`), and a registration for the id with another key is refused (`403 realm_key_mismatch`). Nobody can take an id over.
The same key is what the Host proves to the Coordinator and to players (`docs/CONTROL_PROTOCOL.md`), which is how "this Host is the realm in the list" is established.

The private key stays in the Host Manager's storage (section 9) and never enters a log, a diagnostic, a descriptor or the Registry.

## 3. Signed requests

Every mutating request (and the authenticated self read) is signed.

Headers:

| Header | Value |
|---|---|
| `X-Coa-Registry-Version` | `2` |
| `X-Coa-Realm` | the realm id; for every endpoint except `register` it must equal the id in the path |
| `X-Coa-Timestamp` | unix seconds (decimal) |
| `X-Coa-Signature` | base64url (no padding, 86 characters) of the 64-byte Ed25519 signature |

The signed input is these seven lines, joined by a single `\n` (no trailing newline), as UTF-8 bytes:

```
coa-registry-sig-v2
<protocol version, decimal>
<HTTP method, upper case>
<request path exactly as sent; no query string is allowed>
<realm id>
<unix timestamp, decimal>
<SHA-256 of the body bytes as sent, lower-case hex>      (the empty body for a GET)
```

The signature is made over those bytes with the realm's Ed25519 key (RFC 8032, deterministic). Verification is strict (`verify_strict`). The domain line changed from `-v1` to `-v2`, so a
signature made for protocol 1 can never be valid for protocol 2.

### Rules checked, in this order

1. `429 rate_limited`: the source address is over its allowance (section 7).
2. `413 request_too_large`: more than 65536 body bytes.
3. `400 malformed_request`: a header is missing or malformed; the path has a query string; the body is not the expected JSON, or has an unknown field.
4. `400 unsupported_protocol_version`: the header, or `protocol_version` in the body, is not `2`.
5. `400 invalid_realm_id`: not a canonical UUIDv7, or the header and the path (or body) name different realms.
6. `401 bad_timestamp`: the timestamp is more than 300 s from the Registry's clock (either way).
7. For `register`: `400 invalid_metadata` (a limit or consistency rule of section 6), then the signature is verified with the key *in the body*.
   For the others: `404 unknown_realm`, then the signature is verified with the *stored* key.
8. `401 invalid_signature`: the signature does not match (a forged request also costs its sender more of its address allowance).
9. `403 realm_key_mismatch`: `register` for a known id with another key (cannot happen with a valid signature from the stored key).
10. `401 timestamp_not_monotonic`: the timestamp is not **strictly later than the last request applied for this realm**. This is the replay protection: a captured
    request cannot be sent again, and one delayed past a later request is refused. The Host never reuses a second: it signs `max(clock, last + 1)`.
11. `429 rate_limited` per realm (writes only): 4 immediately, then one every 10 s.

### Test vectors

Key: the Ed25519 seed is 32 bytes of `0x07`; public key (base64url) `6kpsY-KcUgq-9VB7Ey7F-ZVHdq6-vnuSQh7qaRRG0iw`.
Realm `018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d`.

Heartbeat, body `{"protocol_version":2}` (SHA-256 `e68cb099ab8a3557572bff7e669270ac75fe321c1920b12a2aa7a535d1d16155`), timestamp `1790000000`:

```
coa-registry-sig-v2
2
POST
/registry/v2/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d/heartbeat
018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d
1790000000
e68cb099ab8a3557572bff7e669270ac75fe321c1920b12a2aa7a535d1d16155
```

signature `MjXmYOIM1Bap5C5XNPspElOTNIAnKBN4pTfvhF6r29zq7A0elzXi7Qk8D3FyfL1CtVmpliUAr3MY1TGrGPyqAQ`.

Authenticated self read, `GET /registry/v2/realms/018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d/self`, empty body (SHA-256 `e3b0c442…b855`), timestamp `1790000030`:
signature `yqd_TFTooZcWb0GhLw3Tl9FwnBWCHo3Q2AYpoNn4CCJljENOgW9WaNlODoInEVglrCNKJmoYuahPyZfcEJk_AA`.

Both are checked by `sign::tests` and independently by `tools/registry-vector-check.py` (RFC 8032 reference algorithm in plain Python).

## 4. Endpoints

| | |
|---|---|
| `GET  /registry/v2/healthz` | unauthenticated: `{"status":"ok","protocol_version":2}`, `503 degraded` when the database is unreachable |
| `POST /registry/v2/realms/register` | create the realm's record, or announce the same key again; publishes |
| `POST /registry/v2/realms/{realm_id}/heartbeat` | presence, population, and the listing / capabilities when they changed |
| `POST /registry/v2/realms/{realm_id}/unpublish` | stop publishing (the record stays) |
| `GET  /registry/v2/realms/{realm_id}/self` | the authenticated self read (works while unpublished; integration tests, support) |
| `GET  /registry/v2/realms` | **public**, read-only: a page of the realm list (section 5) |
| `GET  /registry/v2/realms/{realm_id}` | **public**, read-only: one published realm in full (section 5) |
| `/registry/v1/*` | refused as described in section 0 |

Anything else is `404`. There is no search endpoint other than the list's `q`, no write on the public side, and no admin endpoint.

Errors are `{"error":{"code":"<code>","message":"…"}}` with the codes above plus `unknown_realm` (404), `not_published` (409), `unavailable` (503, storage), `internal` (500).
Codes a repeat cannot fix are *permanent* (`malformed_request`, `unsupported_protocol_version`, `request_too_large`, `invalid_metadata`, `invalid_realm_id`, `invalid_signature`,
`realm_key_mismatch`): the Host stops instead of retrying.

### The three parts of an advertisement

| Part | Hash | What it is |
|---|---|---|
| **Listing** | `listing_hash` = SHA-256 of `coa-registry-listing-v2\0` ‖ the JSON of the listing in its fixed field order | what a player reads: `display_name`, `description`, `language`, `region` (optional), `rates`, `modules`, `account_provisioning`, `manager_version` |
| **Capabilities** | `capabilities_hash` = `advert_hash` (SHA-256 of `coa-registry-advert-v1\0` ‖ content hash ‖ progression numbers and signature) | the real core's capability profile (profile version 2), what the compatibility check uses; the ruleset and the level cap are *derived* from it, never claimed separately |
| **Population** | none (changes every heartbeat) | `players` (human characters online), `bots` (companion characters online, counted separately), `capacity` (the worldserver's `PlayerLimit` when above 0, else absent) |

Every number comes from the real server, and what cannot be known is absent/`null`, never a guess: rates from the realm's effective `worldserver.conf` (`Rate.XP.Kill`, `Rate.XP.Quest`,
`Rate.XP.Explore`, `Rate.Drop.Item.Normal`, `Rate.Drop.Money`, `Rate.Reputation.Gain`, `Rate.Honor`), modules from the Manager's module catalog against the server's installed and enabled modules
(`{id, version?, enabled}`; no descriptions are sent, a player's Manager brings its own catalog), population from the realm's database. There is **no `game_mode` field and no ping field**:
a ping needs a game route, which does not exist yet.

### Register

```json
{ "protocol_version": 2, "realm_id": "…", "public_key": "<base64url, 43 chars>",
  "listing": { "display_name": "…", "description": "…", "language": "en", "region": "EU",
               "rates": { "xp_kill": 1.0, "xp_quest": 1.0, "xp_explore": 1.0, "loot": 1.0, "money": 1.0, "reputation": 1.0, "honor": 1.0 },
               "modules": [ { "id": "mod-ah-bot", "version": "1.2.0", "enabled": true } ],
               "account_provisioning": { "automatic": true, "existing_only": false },
               "manager_version": "0.6.6" },
  "listing_hash": "<64 hex>",
  "capabilities": { …RealmCapabilities profile version 2… }, "capabilities_hash": "<64 hex>",
  "population": { "players": 3, "bots": 12, "capacity": 100 } }
```

Response: `created`, `metadata_revision`, `published`, `listing_hash`, `capabilities_hash`, `server_time`, `heartbeat_interval_secs` (30), `online_ttl_secs` (120).
Registering a known id with the same key is idempotent and is how a restarted Manager (or a republication after `unpublish`, or after protocol 1) announces itself.

### Heartbeat

`listing_hash`, `capabilities_hash` and `population` are always sent; `listing` and `capabilities` only when their hash differs from the one the Registry last acknowledged. If a hash differs and
its part was not sent, the answer carries `resend_listing` / `resend_capabilities: true` and nothing of it is applied; the Host sends the part with the next heartbeat. An unchanged heartbeat
writes only `last_seen_at`, the population and the replay counter. A heartbeat of an unpublished realm is `409 not_published`; the Host registers again to publish.

### Metadata revision

`metadata_revision` starts at 1 and grows by one whenever the listing, the capabilities or the published state changes; presence and population do not move it.

## 5. The public read API (Phase 11)

`GET /registry/v2/realms` returns `{ "protocol_version": 2, "realms": [RealmSummary…], "next_cursor": "…"|null, "server_time": … }`. **Bounded by construction**:

| Parameter | Rule |
|---|---|
| `limit` | 1..100, default 50 |
| `cursor` | opaque keyset cursor from the previous page (≤ 300 bytes); it binds the sort, the order and the filters that made it, and a cursor used with other ones is refused. No offsets, no totals |
| `q` | substring of the display name, ≤ 64 characters, case-insensitive (`strpos`, so `%` and `_` are ordinary characters) |
| `ruleset` | `coa` or `wildcard` |
| `cap_min`, `cap_max` | level cap range, 0..255 (realms that did not report a cap match only when no cap filter is set) |
| `module` | a module id that is present **and enabled** |
| `players_min` | at least this many human players online |
| `language`, `region` | exact tags |
| `status` | `online` (default), `offline`, `all`; online = published and heard of within the TTL |
| `sort`, `order` | `name` (asc by default), `players`, `cap`, `created` (desc by default), `asc`/`desc`; every sort is totalled by `realm_id`, so pages never repeat or skip a realm |

Any other parameter, a repeated one or a query string over 1024 bytes is `400`. Only published, protocol-2 realms are ever listed. `RealmSummary` carries what the table shows (name, a description
cut to 160 characters, language, region, ruleset and level cap derived from the capabilities, rates, modules, population, account provisioning, `capabilities_hash`, `online`, `last_seen_at`);
`GET /registry/v2/realms/{id}` adds the full description, the public key (so that a player's Manager can check the Host against it), the full capabilities and the revision.
An unpublished or unknown realm is `404`.

Answers carry `Cache-Control: public, max-age=5` and a strong `ETag` (a conditional request with a matching `If-None-Match` is `304`); the service also keeps a built answer for 5 s so that many
clients opening the list do not each reach the database. The list is served from partial indexes that match every sort (`migrations/0003`), so a page is an index range scan with a `LIMIT`
even with tens of thousands of realms (checked live with 5000 synthetic realms; see `docs/REGISTRY_DEPLOYMENT.md`). Each source address has its own allowance for reads (burst 60, 2 per second).

## 6. Limits

| What | Limit |
|---|---|
| request body | 65536 bytes |
| display name | 1..80 characters, no control characters, no direction overrides (U+202A–202E, U+2066–2069, U+200E/F), no surrounding space |
| description | 1024 bytes (UTF-8); control characters other than `\n` refused; plain text, never HTML |
| language / region | short tags (`en`, `pt-BR`; `EU`), ≤ 16 bytes, letters digits and `-` |
| Manager version | ≤ 32 bytes of `[A-Za-z0-9.+_-]` |
| rates | each 0..1000 or absent |
| modules | ≤ 64, each id `[a-z0-9-]` ≤ 64 characters, a plain version ≤ 32 bytes or absent, no duplicates |
| population | each number ≤ 100000, players ≤ capacity |
| account provisioning | at least one of `automatic`, `existing_only` |
| capabilities | profile version 2 only; field by field (≤ 64 extensions, ≤ 16 collection kinds, ≤ 8 client tables, tokens ≤ 64 characters, hashes 64 hex), unknown fields refused, `content_profile_hash` must equal the hash of the content |

The advertised capabilities type mirrors `RealmCapabilities` field for field (same names, same order: the content hash is taken over the JSON); `coa-core` tests that a real
profile parses into it and keeps its hash, so the two cannot drift apart unnoticed. Consumers must render every string as plain text (names, descriptions and module ids are not trusted).

## 7. Presence and abuse controls

* Heartbeat every ≈30 s (±10 % jitter); online = published and heard of within **120 s**. Missing heartbeats make a realm offline; they never delete it.
* A Host sends heartbeats only while its realm is running; a stopped realm goes offline after the TTL.
* Per source address: 120 requests/minute (burst 120); registration: burst 10, 10 per hour (production defaults; the staging gates ran with 100). A forged request costs 5.
  The address is the last entry of `X-Forwarded-For`, which Caddy sets to the client address it saw; the Registry publishes no port, so only Caddy can reach it.
* Per realm: heartbeat and unpublish burst 4, one more every 10 s (applied only after the signature was verified, so forged traffic cannot throttle a real realm).
* 64 requests in flight, 256 open connections, 10 s request timeout, database pool of 8, `statement_timeout` 5 s, `lock_timeout` 3 s, a 70 KB body cap at Caddy as well.

## 8. Storage

PostgreSQL, forward-only migrations (`migrations/NNNN_name.sql`, recorded in `schema_migrations`; a database that is ahead of the binary is refused):

| Migration | |
|---|---|
| `0001_realms` | the `realms` table (identity, key, timestamps, published, metadata, capabilities JSONB and hash, revision, last request timestamp, player numbers) |
| `0002_protocol_v2` | `advert_version`, the structured listing columns (region, rates, modules, account provisioning, players, bots, level cap, …) and `listing_hash`; protocol 1 rows stay with `advert_version = 1` |
| `0003_browse_indexes` | partial indexes (published, protocol 2) on exactly the expressions the list sorts by |
| `0004_created_whole_seconds` | `created_at` is a whole second (a CHECK), so that the `created` cursor is exact |

There is no heartbeat history and no table about players, characters, accounts or relays.

## 9. The Host side

`crates/coa-core/src/realm_registry`:

* **Identity**: created the first time publishing is enabled for a local realm. The key is behind `KeyStore`; the implementation (`FileKeyStore`) keeps
  `<data>/registry/keys/<realm id>.key` (base64url of the 32-byte seed), created with its permissions restricted *before* the secret is written: mode 0600 on Unix, and
  on Windows the inherited permissions removed and the current user granted full control (`icacls`). **Limitation:** this is a file; it is weaker than DPAPI, which protects the *player's*
  secrets in Phase 12 (`SecretStore`) but not yet the realm key; anybody who can read the user's files can read it. It is never written to the realm descriptor, `registry.json`, a log, the
  diagnostics or the status shown to the interface. If the key is missing, publication stops with a message; publishing again makes a *new* identity (it never guesses the old key).
* **Settings**: `<data>/registry/registry.json` (the Registry address, and per local realm: enabled, realm id, name, description, language, region, and the owner's access choices of
  `docs/CONTROL_PROTOCOL.md`: *existing accounts only* and a stated route).
* **Lifecycle** (`RegistryHost` on its own thread, `RegistryRuntime`): publish → ensure identity → register → heartbeat every ~30 s; a changed listing or capabilities rides the next heartbeat;
  a Manager restart resumes with the same id and key; unpublish is retried in the background until delivered or refused. The loop only *reads* what a realm advertises (probing is done on a
  separate thread and cached), so a Registry that is down, slow or hostile cannot touch the worldserver or any portable session.
* **Retry**: transient failures (network, timeout, 5xx, rate limit) back off 5 s → 300 s with ±25 % jitter. `unknown_realm`/`not_published` register again. Clock refusals retry with backoff up to 20 times.
  Permanent refusals stop the realm's publication (`rejected`) until the owner presses *Try again* or republishes.
* **Interface**: *Settings → Public listing* of a server (Registry address, name, description, language, region, *Publish* / *Stop publishing*, status, and the access options).

## 10. The player side (Phase 11)

*Servers* is the first page of Player Mode (and available to a Host): a table of the public list (Server, Modules, Cap, Rates, Mode CoA/Wildcard, Ping, Online "N players + M bots"), with search,
filters (mode, level cap range, module, online players) and sortable columns (name, cap, online); the detail drawer shows the rates, the modules and whether the *selected portable character*
can play there, using the existing Phase 7/8 compatibility logic against the capabilities in the record, and the join panel of `docs/CONTROL_PROTOCOL.md`. Module names and tooltips come from the
**local** module catalog; an id the catalog does not know is shown as plain text. Ping shows "—" with an explanation until a real route exists. The Registry address is a setting (and
`COA_REGISTRY_URL` for tests and deployments); nothing is hard-coded. The browse client sends nothing about the player.

## 11. Not in this document

The control plane (accounts, character claiming, the Coordinator), Relay, direct-connect optimisation, portable-character transfer to another Manager. **HTTPS**: the staging node serves plain HTTP
(no domain, so no certificate); requests are signed and carry no secret, and the public list is public. TLS is a hard requirement before the list is shown to players outside the test
group, and is the one open item of Phase 11 (see `docs/REGISTRY_DEPLOYMENT.md`). No custom PKI is used or planned.
