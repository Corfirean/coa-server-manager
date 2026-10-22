# Multi-Ruleset Realms & Portable Characters: Architectural Audit

This audit evaluates the architecture to support three independent, strictly isolated gameplay rulesets:
1. **Conquest of Azeroth (`coa`)**
2. **Wildcard (`wildcard`)**
3. **Warcraft Reborn (`reborn`)**

Date: October 2026  
Status: Updated for MR1.1 Gate (Post-Foundation Fixes)

---

## 1. Executive Summary: Baseline vs Implemented vs Planned

### 1.1 Baseline Discovered Before Implementation (v0.7.0 Master)
- `realms::Mode` defined `Coa`, `Wildcard`, and `Reborn` for local realm profiles and database switching (`acore_characters`, `acore_characters_wildcard`, `acore_characters_reborn`).
- The **Portable Characters and Public Realm Registry subsystems** were designed almost entirely around `coa`, with experimental/preliminary partial awareness of `wildcard`, and an explicit blocking error for `reborn` (`ruleset_of`: *"Portable characters are not supported on Warcraft Reborn realms yet"*).
- SQLite `character` table had `CHECK (ruleset IN ('coa', 'wildcard'))`, which hard-failed on any `reborn` row.
- SQLite `collection` table was keyed by `(profile_id, kind)` without `ruleset`, causing cross-ruleset collision.
- `Request::TransferOffer`, `PortableSessionStarted`, `PortableCheckpoint`, `OwnerAck`, `CollectionObserved`, `CollectionState`, and `CollectionAck` lacked explicit protocol-level `ruleset` fields or version negotiation.
- Collection validation used unsafe prefix checks (`kind.starts_with(ruleset)`), permitting prefix confusion attacks.
- `Ruleset::schema(kind)` contained a fallback `_ => "acore_characters"`, routing unknown database kinds to CoA.

### 1.2 Implemented in 58a99f0+ and MR1.1
- **Authoritative Ruleset Enum:** `Ruleset` (`Coa`, `Wildcard`, `Reborn`) unified across `coa-core`, `coa-registry-proto`, `coa-control-proto`, and frontend models.
- **Transactional SQLite Migration 009:** Migrates released v0.7.x databases by recreating both `collection` (composite key `profile_id, ruleset, kind`) and `character` (CHECK constraint allowing `'coa', 'wildcard', 'reborn'`) with FK toggles ensuring zero child row loss.
- **Protocol Versioning & Negotiation:**
  - Control protocol bumped to `CONTROL_PROTOCOL_VERSION = 2`. `Request::TransferOffer` requires `ruleset: String` (no default).
  - Session protocol bumped to `SESSION_PROTOCOL_VERSION = 3`. `SessionOffer`, `PortableSessionStarted`, `PortableCheckpoint`, and `OwnerAck` require explicit `ruleset` (no serde defaults). Old v2 messages require explicit legacy translation.
  - Collection protocol set to `COLLECTION_PROTOCOL_VERSION = 2`. `CollectionObserved`, `CollectionState`, and `CollectionAck` require explicit `ruleset`.
- **Schema Safety:** Unsafe fallback in `Ruleset::schema(kind)` removed; unknown database kinds return a hard error.
- **Exact Collection Namespace Validation:** `carried_kind_for(kind, ruleset)` strictly validates allowed collection kinds; prefix confusion attacks (`coax:appearance`, etc.) are rejected with zero mutation.
- **Multi-Realm Local Key Resolution:** `find_access` resolves composite keys `srv-<install_id>:<ruleset>` while retaining `srv-<install_id>` for existing single-realm publication until MR4.

### 1.3 Still Planned (Subsequent Milestones)
- **MR2 (Wildcard Vertical Slice):** Full end-to-end import/export, session runtime, and live verification of Wildcard gameplay.
- **MR3 (Warcraft Reborn Vertical Slice):** Unlocking `ruleset_of` for Reborn, import/export adapters, and live verification.
- **MR4 (Multi-Realm Publication UI & Registry):** Simultaneous publication of multiple RealmIds from a single Manager host under distinct identities and keypairs.

---

## 2. Inventory of Current Subsystem Locations & Representation

### 2.1 Realm Profiles & Databases (`crates/coa-core/src/realms.rs`, `db.rs`)
| Ruleset / Mode | Realm ID | Character Database | World Database | Auth Database | Configuration Profile |
|---|---|---|---|---|---|
| **CoA** | `1` | `acore_characters` | `acore_world` | `acore_auth` | `Settings/realm-profiles/coa.json` |
| **Wildcard** | `2` | `acore_characters_wildcard` | `acore_world_wildcard` | `acore_auth` | `Settings/realm-profiles/wildcard.json` |
| **Warcraft Reborn** | `3` | `acore_characters_reborn` | `acore_world_reborn` | `acore_auth` | `Settings/realm-profiles/reborn.json` |

- **Simultaneous Startup Policy:**
  - CoA + Wildcard simultaneous is supported via `secondary_world_port` and `secondary_ra_port` (CoA on primary world port 8085, Wildcard on secondary e.g. 8086).
  - Warcraft Reborn simultaneous startup is **strictly unsupported** (`s.simultaneous && s.active == Mode::Reborn` returns `Error::Invalid("Simultaneous startup supports CoA and Wildcard only.")`).
- **Worldserver Capability Detection:**
  - Executable inspected directly (`Core/worldserver.exe` on Windows repack, `Core/worldserver` on Linux Docker).
  - Wildcard detected by byte pattern: `b"Wildcard synergy settings"`.
  - Reborn detected by byte pattern: `b"Warcraft Reborn class spells are unavailable"`.

### 2.2 Ruleset Enumeration & Protocols
- `crates/coa-core/src/realms.rs`: `enum Mode { Coa, Wildcard, Reborn }`
- `crates/coa-core/src/portable/model.rs`: `enum Ruleset { Coa, Wildcard }` *(Lacks `Reborn`)*
- `crates/coa-registry-proto/src/caps.rs`: `enum Ruleset { Coa, Wildcard }` *(Lacks `Reborn`)*
- `services/coa-registry/src/pg.rs`: `ruleset: match ruleset.as_str() { "coa" => Ruleset::Coa, "wildcard" => Ruleset::Wildcard }` *(Lacks `Reborn`)*
- `src/lib/browse.ts`: `export type Ruleset = "coa" | "wildcard";` *(Lacks `reborn`)*

### 2.3 Public Registry Model & Publication
- `crates/coa-core/src/realm_registry/settings.rs`: `RegistrySettings.realms` is a `BTreeMap<String, PublishConfig>`.
- `src/screens/PublishCard.tsx`: Uses `localId = 'srv-' + serverId`.
- **Host vs Published Realm Identity Issue:**
  One installation (`InstallMeta.id`) hosting multiple rulesets currently shares `localId = srv-<id>`. If the host switches active realm from CoA to Wildcard, or publishes both under simultaneous mode, `srv-<id>` cannot represent both published realms distinctly.
  Each published realm must possess its own distinct local realm identity (`srv-<id>-<ruleset>` or `srv-<id>:<ruleset>`), unique `RealmId` (UUIDv7), and independent Ed25519 signing key in `KeyStore`.

### 2.4 Canonical Portable Storage (`crates/coa-core/src/portable/store.rs`)
- SQLite schema (`%LOCALAPPDATA%\CoAServerManager\portable\portable.db`):
  - `character`: `ruleset TEXT NOT NULL CHECK (ruleset IN ('coa', 'wildcard'))` -> Must be updated to allow `'reborn'`.
  - `collection`: `PRIMARY KEY (profile_id, kind)` -> Unpartitioned by ruleset! Must become `(profile_id, ruleset, kind)`.
  - `character_server_mapping`: `PRIMARY KEY (character_id, server_id), UNIQUE (server_id, local_guid)`.
  - `item_mapping`: `PRIMARY KEY (character_id, server_id, portable_item_id)`.
  - `pet_mapping`: `PRIMARY KEY (character_id, server_id, portable_pet_id)`.
  - `owner_session`: `character_id`, `server_id`, `c0_revision`, `b0_hash`.
  - `host_session`: `character_id`, `server_id`, `local_guid`.

### 2.5 Remote Transfer & Control Protocol (`crates/coa-control-proto/src/app.rs`)
- `Request::TransferOffer`:
  `{ transfer_id, character_id, canonical_revision, content_hash, total_size, collections }`
  -> **Missing `ruleset`**. An offer must bind `ruleset: Ruleset` so the host can reject before chunk transport or preflight if target realm mismatch occurs.
- `Request::TransferCommit`:
  Currently relies on cached transfer metadata.
- `Response::TransferCommitted`:
  Returns session parameters.

### 2.6 Runtime Sessions & Checkpoints (`crates/coa-core/src/portable/session/protocol.rs`)
- `PortableSessionStarted`: `{ protocol_version, session_id, character_id, server_id, base_canonical_revision, baseline_generation, b0, content_hash, progression }`
  -> **Missing `ruleset`**.
- `PortableCheckpoint`: `{ protocol_version, session_id, character_id, server_id, base_canonical_revision, sequence, final_checkpoint, realm_snapshot, content_hash, pin }`
  -> **Missing `ruleset`**.
- `OwnerAck`: `{ protocol_version, session_id, sequence, outcome, canonical_revision, canonical_hash, owned_items, owned_pets, canonical, next_session, pin }`
  -> Must verify ruleset match in owner session state before applying 3-way merge.

### 2.7 Direct & Relay Routing
- **Direct Ingress Proxy (`crates/coa-core/src/control/direct_ingress.rs`):**
  Listens on direct auth and world ports, intercepts `CMD_REALM_LIST` from the Auth server, and rewrites the realm entry address to the public direct world IP and port. When multiple realms are active (simultaneous CoA + Wildcard), each published realm must target its respective local world port (`ports.world` vs `s.secondary_world_port`).
- **Relay Hub & Allocations (`services/coa-relay/`, `crates/coa-core/src/control/relay_link.rs`):**
  Relay allocation requests are authenticated by Ed25519 realm key. Allocations bind a short-lived token to an isolated auth/world stream pair.

---

## 3. Strict Isolation Invariants

The fundamental architectural invariant is:
$$\text{portable\_character.ruleset} == \text{target\_realm.ruleset}$$

On mismatch:
1. **Zero DB Mutation:** No rows written to `acore_characters*`, `item_instance`, or modules.
2. **Zero Session Creation:** No `host_session` or `owner_session` record opened or armed.
3. **Zero Mapping Creation:** No entry in `character_server_mapping`, `item_mapping`, or pet mapping.
4. **Zero Collection Merge:** No union into collections of a different ruleset.
5. **Zero Canonical Revision Change:** Canonical revision remains unchanged; reject with explicit mismatch error.
6. **No Implicit Conversion:** Never attempt conversion, level projection across rulesets, or guessing missing rulesets.

---

## 4. Execution Plan & Phasing

1. **Authoritative RulesetId Abstraction (Section 3):**
   - Normalize `RulesetId` / unified enum across `coa-core`, `coa-registry-proto`, `coa-control-proto`, and TypeScript models.
   - Values: `coa`, `wildcard`, `reborn`.
2. **Partitioning Collections & Canonical Storage (Sections 8, 11):**
   - Add SQLite migration `009_multiruleset_collections.sql` partitioning collections by `(profile_id, ruleset, kind)` and expanding `character` table ruleset constraint to allow `'reborn'`.
   - Update `Store` APIs to accept `ruleset: Ruleset` on character operations and collection operations.
3. **Registry & Multi-Realm Publication on One Host (Sections 5, 6, 26):**
   - Partition published realm keys: `srv-<serverId>:<ruleset>`.
   - Each published realm receives its own `RealmId`, Ed25519 key, heartbeat, status, and capabilities.
   - Registry PostgreSQL migration for `reborn` ruleset check.
4. **Remote Transfer Envelope & Session Binding (Sections 14, 22):**
   - Add `ruleset` to `TransferOffer`, `PortableSessionStarted`, `PortableCheckpoint`.
   - Enforce 4-way equality: envelope ruleset == payload ruleset == published realm ruleset == host target DB ruleset.
5. **Wildcard Vertical Slice (Section 15):**
   - End-to-end testing of Wildcard claiming, transfer, session, checkpoint, and collection partitioning.
   - Verify negative cross-ruleset attack scenarios.
6. **Warcraft Reborn Audit & Vertical Slice (Sections 18, 19):**
   - Complete `docs/REBORN_PORTABLE_AUDIT.md`.
   - Wire Reborn adapter into the unified engine and verify vertical slice.
7. **UI Multi-Ruleset Filtering (Sections 7, 27):**
   - Server browser ruleset tabs/filters (`All`, `Conquest of Azeroth`, `Wildcard`, `Warcraft Reborn`).
   - Portable character picker filtered by target realm ruleset.
8. **Regression & Cross-Ruleset Attack Test Suite (Section 29):**
   - Comprehensive unit and integration attack tests ensuring strict isolation.
