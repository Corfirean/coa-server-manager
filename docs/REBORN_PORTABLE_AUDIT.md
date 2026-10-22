# Warcraft Reborn Portable Characters Audit & Semantic State

## 1. Historical Baseline
The released v0.7.0 master branch codebase contained foundational support for local realm execution:
- Mode: `Mode::Reborn`
- Realm ID: `3`
- Databases: characters DB `acore_characters_reborn`, world DB `acore_world_reborn`, shared auth DB `acore_auth`
- Policy: Reborn runs as the active primary realm on the main world port (8085). Simultaneous execution with CoA/Wildcard is strictly unsupported (`"Simultaneous startup supports CoA and Wildcard only."`).

Before the multi-ruleset implementation, portable character transfers into Warcraft Reborn were temporarily blocked by:
```rust
// crates/coa-core/src/portable/realm/mod.rs
Mode::Reborn => Err(PortableError::Invalid("Portable characters are not supported on Warcraft Reborn realms yet.".into())),
```

## 2. Current MR3 Implementation
In MR1.1, MR2, and MR3, generic multi-ruleset infrastructure was completed and enabled for Reborn:
- **Ruleset Identity**: `Ruleset::Reborn` established across `coa-core`, `coa-registry-proto`, `coa-control-proto`, and frontend TypeScript models (`"reborn"` wire value, display name `"Warcraft Reborn"`).
- **SQLite Storage**: Migration `009_multiruleset.sql` upgrades released databases to permit `reborn` in `character` CHECK constraints without dropping existing character rows or foreign keys.
- **Collections**: `reborn:appearance` and `reborn:vanity` defined and carried under allowlist in `carried_kind_for`. Partitioned under `(profile_id, 'reborn', kind)`.
- **Wire Protocols**: Control Protocol v2, Session Protocol v3, and Collection Protocol v2 bind `ruleset: Ruleset` explicitly in all messages.
- **Fail-Closed Routing & Ingress**: `srv-X:reborn` resolves strictly to the Reborn realm and fails closed if inactive. HostService rejects malformed server-id ruleset suffixes.
- **Automated Verification**: End-to-end claiming, export, snapshot import, runtime session lifecycle, intermediate/final checkpoints, and logout sync pass.
- **Three-Space Isolation**: Torture fixtures prove zero mutation across rulesets:
  - Reborn character -> CoA / Wildcard realm (rejected with zero mutation).
  - CoA / Wildcard character -> Reborn realm (rejected with zero mutation).
  - Reborn collection rejects foreign namespaced entries (`coa:*`, `wildcard:*`).

## 3. Actual Reborn Semantic-State Audit

### 3.1 Gameplay Identity and Class Architecture
Warcraft Reborn in this core is configured via `CoA.ClassModel = "coa wcr"` / `wcr` mode:
- Classes in Warcraft Reborn are standard classic WoW classes (Warrior, Paladin, Hunter, Rogue, Priest, Death Knight, Shaman, Mage, Warlock, Druid; class IDs 1..11) rather than CoA custom classes (Barbarian, Witch Doctor, etc., IDs 12..32).
- Reborn spells, class abilities, talent trees, and creature summons use dedicated spell/creature ID ranges and corrections (e.g. `REBORN_CREATURE_OFFSET = 1100000`, spell twin scripts), which operate on standard AzerothCore character persistence structures.
- Reborn characters use the exact same character persistence schema (`acore_characters_reborn`) as vanilla/CoA character databases, cloned directly via `db.clone_structure("acore_characters", &stage_chars, &cache)` in `crates/coa-core/src/realms.rs`.

### 3.2 Table & State Classification Matrix

| Category / State | Database Table(s) | Portable Character Mapping | Classification |
| :--- | :--- | :--- | :--- |
| **Identity (name, race, class, gender)** | `characters` | `PortableCharacter.name`, `.race`, `.class`, `.gender` | **Already covered by generic PortableCharacter** |
| **Progression (level, XP, money)** | `characters` | `Progression.level`, `.xp`, `.money` | **Already covered by generic PortableCharacter** |
| **Inventory & Equipment** | `character_inventory`, `item_instance` | `Item` records, container links, slot indices, item entry | **Already covered by generic PortableCharacter** |
| **Enchants, Gems, Random Properties** | `item_instance` (`enchantments`, `randomPropertyId`, `durability`) | `Item.enchantments`, `random_property_id`, `durability` | **Already covered by generic PortableCharacter** |
| **Bags & Bank** | `character_inventory` (bag slots, bank slots) | `Item` container slots; personal bank slots | **Already covered by generic PortableCharacter** |
| **Spells** | `character_spell` | `PortableCharacter.spells` (array of spell IDs) | **Already covered by generic PortableCharacter** |
| **Talents** | `character_talent` | `PortableCharacter.talents` (spec, talent IDs, ranks) | **Already covered by generic PortableCharacter** |
| **Glyphs** | `character_glyphs` | `PortableCharacter.glyphs` (spec, slot, glyph ID) | **Already covered by generic PortableCharacter** |
| **Action Bars** | `character_action` | `PortableCharacter.actions` (button, action, type) | **Already covered by generic PortableCharacter** |
| **Skills & Weapon proficiencies** | `character_skills` | `PortableCharacter.skills` (skill ID, value, max) | **Already covered by generic PortableCharacter** |
| **Quests (Active & Rewarded)** | `character_queststatus`, `character_queststatus_rewarded` | `Quest` records, objective counters, rewarded list | **Already covered by generic PortableCharacter** |
| **Reputation** | `character_reputation` | `Reputation` records (faction ID, standing, flags) | **Already covered by generic PortableCharacter** |
| **Pets & Pet Spells** | `character_pet`, `pet_spell` | `Pet` records, entry, name, spells | **Already covered by generic PortableCharacter** |
| **Character Settings & Keybindings** | `character_settings`, `character_account_data` | `Setting` records (source, data payload) | **Already covered by generic PortableCharacter** |
| **Vanity / Transmog Collections** | `account_appearance_collection`, `account_vanity_collection` | `reborn:appearance`, `reborn:vanity` collections | **Account-wide Reborn collection** |
| **Active Auras / Buffs** | `character_aura` | Not persisted in portable snapshot | **Realm-local and intentionally not portable** (refreshed on login) |
| **Spell Cooldowns** | `character_spell_cooldown` | Not persisted in portable snapshot | **Realm-local and intentionally not portable** |
| **Daily / Weekly Quest Resets** | `character_queststatus_daily`, `weekly`, `monthly` | Not persisted in portable snapshot | **Realm-local and intentionally not portable** |
| **Instance Bindings** | `character_instance`, `character_homebind` | Not persisted in portable snapshot | **Realm-local and intentionally not portable** |
| **Battleground / Arena Ratings** | `character_arena_stats`, `character_battleground_random` | Not persisted in portable snapshot | **Realm-local and intentionally not portable** |
| **Manastorm / Challenge states** | `ascension_manastorm_*`, `coa_character_challenge` | Classified as `Blocking` if active | **Blocking** (blocks export while active challenge exists) |
| **Custom Reborn Tables** | None exist in database | N/A | **Irrelevant** (no custom tables in `acore_characters_reborn`) |

### 3.3 Reborn Extension Requirement Decision

```text
Reborn extension required: NO
```

**Rationale:**
1. Warcraft Reborn builds persistent character state entirely within standard AzerothCore character tables (`characters`, `character_spell`, `character_talent`, `character_action`, `character_skills`, `character_inventory`, `item_instance`, `character_reputation`, `character_pet`).
2. There are no separate Reborn-specific character tables in `acore_characters_reborn` (the schema is identical to `acore_characters`).
3. Reborn class abilities, spells, and talent configurations are encoded natively via standard spell IDs and talent IDs supported by `PortableCharacter.spells` and `PortableCharacter.talents`.
4. Character settings (such as build selections or client UI configs) persist through `character_settings`, which is already captured in `PortableCharacter.settings`.
5. Transmog and cosmetic appearance selections are synchronized cleanly via the `reborn:appearance` and `reborn:vanity` account collections.
6. Therefore, adding a custom typed extension is not needed and would introduce redundant schema without semantic benefit. The generic `PortableCharacter` model provides 100% semantic continuity for Reborn character builds.

## 4. Remaining Manual Verification
- **Combined Physical Real-Client Gate**:
  - Live client connection to a running Warcraft Reborn realm.
  - Character creation on Reborn, progression, claiming to Manager.
  - Transfer/import to a secondary Reborn realm and runtime session verification.
  - Verification that CoA and Wildcard realms remain completely isolated.
  - Status: **PENDING COMBINED MANUAL GATE**.
