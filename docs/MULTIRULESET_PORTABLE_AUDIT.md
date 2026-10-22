# Multi-Ruleset Realms & Portable Characters: Architectural Audit

This audit evaluates the architecture to support three independent, strictly isolated gameplay rulesets:
1. **Conquest of Azeroth (`coa`)**
2. **Wildcard (`wildcard`)**
3. **Warcraft Reborn (`reborn`)**

Date: October 2026  
Status: Updated for MR2 Gate (Wildcard Vertical Slice Completed)

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

### 1.2 Implemented in MR1, MR1.1, and MR2
- **Authoritative Ruleset Enum:** `Ruleset` (`Coa`, `Wildcard`, `Reborn`) unified across `coa-core`, `coa-registry-proto`, `coa-control-proto`, and frontend models.
- **Transactional SQLite Migration 009:** Migrates released v0.7.x databases by recreating both `collection` (composite key `profile_id, ruleset, kind`) and `character` (CHECK constraint allowing `'coa', 'wildcard', 'reborn'`) with FK toggles ensuring zero child row loss.
- **Fail-Closed Realm Backend:** Removed unsafe `unwrap_or(Ruleset::Coa)` in `LocalBackend::info()`. Unreadable, unknown, or broken realms fail closed immediately with error rather than falling back to CoA.
- **Control Protocol 2 & Strict Envelopes:**
  - `CONTROL_PROTOCOL_VERSION = 2`.
  - `Request::TransferOffer` requires explicit `ruleset: String`.
  - `Response::Claimed` returns explicit `ruleset: String`.
  - Claiming and commit verify that the claimed envelope ruleset matches the decoded payload ruleset and matches target realm ruleset.
  - `HostTransfer` stores `ruleset` in SQLite; `transfer_commit` verifies that canonical JSON hash and ruleset match the offer and destination realm before writing to disk.
- **Session Protocol 3 & Ruleset Enforcement:**
  - `SESSION_PROTOCOL_VERSION = 3` and `COLLECTION_PROTOCOL_VERSION = 2`.
  - `SessionOffer`, `PortableSessionStarted`, `PortableCheckpoint`, and `OwnerAck` require explicit `ruleset`.
  - Any mismatch between character ruleset, offer ruleset, session ruleset, or checkpoint ruleset rejects immediately with zero DB mutation and zero canonical revision advance.
- **Wildcard Collections Isolation:**
  - `wildcard:appearance` and `wildcard:vanity` isolated from `coa:appearance` and `coa:vanity`.
  - Cross-ruleset collection merges strictly rejected by `carried_kind_for(kind, ruleset)`.
- **Database Routing & Three-Space Isolation:**
  - Canonical SQL routes through `Db::query()` and `realm_schema` to `acore_characters_wildcard` and `acore_world_wildcard`.
  - Multiple characters sharing local GUIDs on distinct server IDs (`srv-1:coa`, `srv-1:wildcard`, `srv-1:reborn`) coexist with zero collision in character mappings and item mappings.
  - Mutating Wildcard character state, revisions, or collections leaves CoA and Reborn states 100% byte-for-byte untouched.
- **Direct & Relay Routing for Wildcard:**
  - `world_port_for_mode` dynamically determines active world port (8085 for primary CoA, 8086 for secondary Wildcard in simultaneous mode).
  - Direct Ingress proxy and Relay link dynamically route to the correct realm world port.
  - Direct Ingress Auth rewrite replaces internal realm list address with external mapped Wildcard port (e.g. 48086) and never advertises CoA port 8085.

### 1.3 Still Planned (Subsequent Milestones)
- **MR3 (Warcraft Reborn Vertical Slice):** Unlocking `ruleset_of` for Reborn, import/export adapters, and live verification.
- **MR4 (Multi-Realm Publication UI & Registry):** Simultaneous publication of multiple RealmIds from a single Manager host under distinct identities and keypairs.

---

## 2. Current Implementation Architecture & Historical Baseline Inventory

### 2.1 Current Implementation State (As of MR1, MR1.1, MR2, MR2.1)
- **Authoritative Ruleset Representation:**
  - `crates/coa-core/src/portable/model.rs`: `enum Ruleset { Coa, Wildcard, Reborn }` (strict parsing and schema routing without fallbacks).
  - `crates/coa-registry-proto/src/caps.rs`: `enum Ruleset { Coa, Wildcard, Reborn }`.
  - `services/coa-registry/src/pg.rs`: `ruleset: match ruleset.as_str() { "coa" => Ruleset::Coa, "wildcard" => Ruleset::Wildcard, "reborn" => Ruleset::Reborn, ... }`.
  - `src/lib/browse.ts`: `export type Ruleset = "coa" | "wildcard" | "reborn";`.
  - `crates/coa-core/src/realms.rs`: `enum Mode { Coa, Wildcard, Reborn }`.
- **Wire Protocols & Ruleset Requirements:**
  - **Control Protocol 2:** `CONTROL_PROTOCOL_VERSION = 2`. `Request::TransferOffer` requires explicit, non-default `ruleset: String`. `Response::Claimed` returns explicit `ruleset: String`. `HostTransfer` records ruleset.
  - **Session Protocol 3:** `SESSION_PROTOCOL_VERSION = 3`. `SessionOffer`, `PortableSessionStarted`, `PortableCheckpoint`, and `OwnerAck` require explicit, mandatory `ruleset: Ruleset`.
  - **Collection Protocol 2:** `COLLECTION_PROTOCOL_VERSION = 2`. `CollectionObserved`, `CollectionState`, and `CollectionAck` require explicit, mandatory `ruleset: Ruleset`.
- **Database Routing & Fail-Closed Invariants:**
  - `LocalBackend::info()` and `LocalBackend::access()` strictly enforce:
    $$\text{local\_id ruleset} == \text{RealmAccess.mode} == \text{Db.realm()} == \text{RealmInfo.ruleset}$$
    Any mismatch or malformed/unknown ruleset suffix returns an immediate error.
  - `LocalBackend::world_port()` and `crates/coa-core/src/realms::world_port_for_mode()` return fallible `Result<u16>` without silent fallbacks to 8085. If a realm is not running or not simultaneous, port resolution fails closed and disables Direct and Relay routing for that realm.
- **Canonical Storage (`portable.db`):**
  - Transactional SQLite Migration 009 upgraded `character` table constraint to `CHECK (ruleset IN ('coa', 'wildcard', 'reborn'))` while preserving rows byte-for-byte.
  - `collection` table is partitioned by `(profile_id, ruleset, kind)`.
  - Mappings (`character_server_mapping`, `item_mapping`, `pet_mapping`, sessions) isolate distinct server IDs (`srv-X:coa`, `srv-X:wildcard`, `srv-X:reborn`).

### 2.2 Historical Baseline Inventory (Pre-Implementation State as of v0.7.0 Master)
> [!NOTE]
> The following findings reflect the legacy v0.7.0 baseline discovered before MR1/MR2 implementation and are preserved for historical reference:
- *Historical:* `Ruleset` enum in `model.rs` and `caps.rs` lacked `Reborn`.
- *Historical:* `character` table had `CHECK (ruleset IN ('coa', 'wildcard'))`, causing SQLite constraint failures on Reborn.
- *Historical:* `collection` table was keyed only by `(profile_id, kind)` without `ruleset`.
- *Historical:* `TransferOffer`, `PortableSessionStarted`, `PortableCheckpoint`, and `CollectionState` lacked explicit protocol-level `ruleset` fields.
- *Historical:* `LocalBackend::info()` had an unsafe fallback `ruleset_of(db).unwrap_or(Ruleset::Coa)`.
- *Historical:* `world_port_for_mode()` fell back to `ports.world` (8085) even when Wildcard was inactive.
- *Historical:* `Ruleset::schema(kind)` had `_ => "acore_characters"`.

### 2.3 Realm Profiles & Databases (`crates/coa-core/src/realms.rs`, `db.rs`)
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

### 2.4 Public Registry Model & Multi-Realm Publication
- `crates/coa-core/src/realm_registry/settings.rs`: `RegistrySettings.realms` is a `BTreeMap<String, PublishConfig>`.
- `src/screens/PublishCard.tsx`: Uses `localId = 'srv-' + serverId`.
- **Multi-Realm Publication on One Host:**
  Each published realm possesses its own distinct local realm identity (`srv-<id>:<ruleset>`), unique `RealmId` (UUIDv7), and independent Ed25519 signing key in `KeyStore`. Direct Ingress proxy and Relay links dynamically route to the respective realm's active world port.

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
