# Warcraft Reborn Portable Characters Audit

## 1. Overview & Baseline vs Current Implementation

### 1.1 Baseline Discovered Before Implementation
The released v0.7.0 master branch codebase contained foundational support for local realm execution (`Mode::Reborn`, realm ID `3`, characters DB `acore_characters_reborn`, world DB `acore_world_reborn`, shared auth DB `acore_auth`).
Portable character transfers into Warcraft Reborn were temporarily guarded by:
```rust
// crates/coa-core/src/portable/realm/mod.rs
Mode::Reborn => Err(PortableError::Invalid("Portable characters are not supported on Warcraft Reborn realms yet.".into())),
```

### 1.2 Implemented in 58a99f0+ and MR1.1
- **RulesetId / Ruleset**: `Ruleset::Reborn` is established across `coa-core`, `coa-registry-proto`, `coa-control-proto`, and TypeScript models (`"reborn"` wire value, display name `"Warcraft Reborn"`).
- **SQLite Portable Storage:** Migration `009_multiruleset.sql` upgrades released databases to permit `reborn` characters in the `character` table CHECK constraint without dropping child records.
- **Collections:** `reborn:appearance` and `reborn:vanity` defined and carried under exact allowlist in `carried_kind_for`. Partitioned under `(profile_id, 'reborn', kind)`.
- **Wire Protocols:** Control protocol v2, Session protocol v3, and Collection protocol v2 bind `ruleset: Ruleset` explicitly in all messages.

### 1.3 Implemented in MR3 Vertical Slice
- Concrete support for Warcraft Reborn character import/export, claiming, session lifecycle, checkpoints, and logout sync.
- `ruleset_of(&reborn_db)` returns `Ok(Ruleset::Reborn)`.
- Reborn character class formatting correctly maps vanilla/standard classes (`reborn:class:1..11`) alongside custom classes.
- Direct ingress and routing respect active primary Reborn realm (realm ID 3, `acore_characters_reborn`, `acore_world_reborn`, port 8085) and fail closed when Reborn is inactive.
- Frontend UI displays `RulesetBadge` with human-facing labels ("Warcraft Reborn", "Wildcard", "Conquest of Azeroth"), ruleset filter tabs in Portable Characters screen, and ruleset-filtered Character Picker in the JOIN drawer.
- Three-space isolation fixture and torture tests prove complete mutation isolation:
  - Reborn character -> CoA realm (rejected with zero DB mutation).
  - Reborn character -> Wildcard realm (rejected with zero DB mutation).
  - CoA character -> Reborn realm (rejected with zero DB mutation).
  - Wildcard character -> Reborn realm (rejected with zero DB mutation).
  - Reborn collections (`reborn:appearance`, `reborn:vanity`) reject foreign namespaced collections.
- Physical external-client smoke is deferred to the combined manual verification gate.

## 2. Ruleset Identity & DB Schemas
- **RulesetId / Ruleset**:
  - `Ruleset::Reborn` exists in `crates/coa-core/src/portable/model.rs` and `crates/coa-registry-proto/src/caps.rs`.
  - Serialized string: `"reborn"`.
  - Display name: `"Warcraft Reborn"`.
  - Associated Realm ID: `3`.
  - Character Database: `acore_characters_reborn`.
  - World Database: `acore_world_reborn`.
  - Auth Database: `acore_auth` (shared across all modes).
- **Collections**:
  - `reborn:appearance`
  - `reborn:vanity`
  - Partitioned under `(profile_id, 'reborn', kind)` in canonical SQLite store (`portable.db`).

## 3. Simultaneous Startup & Multiworld Policy
- As verified in `crates/coa-core/src/multiworld.rs` and `crates/coa-core/src/realms.rs`:
  - `CoA + Wildcard` simultaneous execution is supported.
  - `Reborn` simultaneous execution is explicitly unsupported (`"Simultaneous startup supports CoA and Wildcard only."`).
  - This policy is preserved strictly without changes.

## 4. Enabling Reborn in Portable Architecture
To complete the Reborn vertical slice with full strict isolation:
1. Map `Mode::Reborn => Ok(Ruleset::Reborn)` in `crates/coa-core/src/portable/realm/mod.rs:ruleset_of`.
2. Update tests verifying `ruleset_of(&reborn_db).unwrap() == Ruleset::Reborn`.
3. Verify Reborn claiming, export, import, session runtime, checkpoints, and collection isolation.
4. Verify non-negotiable isolation tests:
   - Reborn character -> CoA realm (rejected).
   - Reborn character -> Wildcard realm (rejected).
   - CoA character -> Reborn realm (rejected).
   - Wildcard character -> Reborn realm (rejected).
