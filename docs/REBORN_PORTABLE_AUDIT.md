# Warcraft Reborn Portable Characters Audit

## 1. Overview & Current State
The master branch codebase already contains foundational support for Warcraft Reborn (`Mode::Reborn`, realm ID `3`, characters DB `acore_characters_reborn`, world DB `acore_world_reborn`, shared auth DB `acore_auth`).
In previous releases (v0.7.0), portable character transfers into Warcraft Reborn were temporarily guarded by:
```rust
// crates/coa-core/src/portable/realm/mod.rs
Mode::Reborn => Err(PortableError::Invalid("Portable characters are not supported on Warcraft Reborn realms yet.".into())),
```

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
