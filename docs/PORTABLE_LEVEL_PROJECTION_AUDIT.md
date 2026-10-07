# Level-cap projection: audit and policy (Phase 8)

Scope: CoA only. Written from the code of the core fork (`feat/portable-session-bridge`, commit 2f6f9f0ca + modules `mod-coa-content-scaling`,
`mod-coa-playerbots`, ...) and from experiments on a real worldserver with `MaxPlayerLevel = 60` against the disposable realm databases.
Nothing in this document is assumed from stock WotLK: each statement names the code or the observation behind it.

**The invariant the whole phase rests on:** the Owner's canonical character is never down-levelled. A projection is a *view* made for one realm; the full
state (`C0`) stays canonical, and a realm only ever holds a working copy `P0` derived from it.

## 1. Where the effective cap comes from, and whether it can move

| Question | Answer | Evidence |
|---|---|---|
| Source of `MaxPlayerLevel` | `worldserver.conf` `MaxPlayerLevel`, read once into `CONFIG_MAX_PLAYER_LEVEL`; range `1..MAX_LEVEL` | `WorldConfig.cpp:302` |
| Can it change while the worldserver runs? | **No.** The key is registered `ConfigValueCache::Reloadable::No`; nothing in the core or any module calls `setIntConfig(CONFIG_MAX_PLAYER_LEVEL)` (grep over `src/` and `modules/`); `mod-coa-content-scaling` itself logs "Progression layout, expansion packs and MaxPlayerLevel cannot be changed at runtime! Server restart required." on a config reload | `CoAContentScaling.cpp:948-955` |
| Does anything clamp a *real* player above the cap at login? | **No.** `Player::LoadFromDB` does `SetUInt32Value(UNIT_FIELD_LEVEL, fields[6])` with no comparison (`PlayerStorage.cpp:5099`); `HandlePlayerLoginFromDB` has no level check; `ObjectMgr::GetXPForLevel` returns 0 above the table (`ObjectMgr.cpp:8126`) and `GetPlayerLevelInfo` silently uses the cap's row for a higher level. The only clamp in the tree is `ClampBotLevel` of `mod-coa-playerbots` (`BotMgr.cpp:360`), which `GiveLevel(cap)` + `XP = 0` + `SaveToDB` for **bots only** (observed: "BotMgr: clamped 'Shaniel' to server level cap 60") | code + experiment E1 |

Consequence: a level-80 canonical character must never be written to a cap-60 realm as level 80 (the core would run it as an over-level player with no
XP table and no stat row of its own); and the cap is a **per-process constant**, so a change of cap is always a restart, which is a session boundary (section 8).

## 2. How `mod-coa-content-scaling` derives its `ProgressionLayout`

* `CoAContentScaling::InitializeLayout()` (`CoAContentScaling.cpp:262`) takes `maxPlayerLevel` from `CONFIG_MAX_PLAYER_LEVEL`, `tbcActive = CoAContentScaling TBC pack
  enabled && pack registered`, `wotlkActive` likewise, and `Progression.Mode` (`Auto` | `Custom` + `ClassicEnd`, `TbcEnd`) from `mod-coa-content-scaling.conf`; then
  `ProgressionLayout::Create(...)` (`ProgressionLayout.cpp`).
* `Create`: cap 80 with both packs and no custom bounds is the **stock identity** (Classic 1-60, TBC 58-70, WotLK 68-80). Otherwise Classic ends at 45 (cap 60) .. 60 (cap 80)
  by linear interpolation, TBC at 55 .. 70, WotLK ends at the cap. Observed on the cap-60 realm: `Classic 1-45, TBC 45-55, WotLK 55-60`.
* `Validate()` accepts only `60 <= MaxPlayerLevel <= 80`. A layout that fails disables scaling (`_enabled = false`, `LOG_FATAL`).
* The layout is **frozen at startup** (`OnLoadCustomDatabaseTable` -> `FinalizeAndInitialize`); `OnAfterConfigLoad(reload=true)` only reloads the
  non-structural options (`LoadReloadableConfig`).
* The module is **not part of the core repository** (it is copied into `modules/`, git-ignored there): the Manager cannot rely on its API, and the core cannot link to it. What the
  module changes in memory is observable through `sObjectMgr->GetItemTemplate()`, which is the surface projection must use.

## 3. Which `ItemTemplate` fields are changed in memory

`ItemBudgetScaler::ScaleAllItems` runs once in `OnStartup` (`CoAContentScaling.cpp:966`, guarded by `_itemsScaled`, skipped when `CoAContentScaling.ScaleItems = 0`
or scaling is disabled). Per template, unless its policy is `PRESERVE` (`ItemBudgetScaler.cpp:306`):

* `RequiredLevel` and `ItemLevel` are replaced by the effective values of the layout;
* only when `statMultiplier < 1`: every `ItemStat[i].ItemStatValue`, `Damage[i].DamageMin/Max`, and `Armor` are multiplied;
* separately (same function of `OnStartup`) `DungeonProgressionRequirements::reqItemLevel` and the LFG dungeons' min/max levels.

**Raw `RequiredLevel` in `item_template` is therefore not the rule.** Observed on the cap-60 realm (`.coascale item`):

| Item | raw `RequiredLevel` | runtime `RequiredLevel` | policy |
|---|---|---|---|
| 19019 Thunderfury | 60 | **41** | `REVIEW_SPECIAL` |
| 1063235 custom dagger | 80 | **60** | `TIER_ALIGNED` |
| 300000 Valiant Holy Champion's Gloves | 80 | **60** | `TIER_ALIGNED` |
| 490 Frosty Gauntlets | 70 | **55** | `TIER_ALIGNED` |
| 2061035 Stormwatcher's Turban (custom) | 80 | **80** | `PRESERVE` |
| 2069385 Legwraps of Eternal Sorrow (custom) | 80 | **80** | `PRESERVE` |

Of 60 random weapons/armor authored above 60, 57 compress to <= 60 and 3 (`CUSTOM_FALLBACK`, `PRESERVE`) keep 70/80; of 250 random cloth/misc armor
authored above 60, 2 keep 80. So on a cap-60 realm **almost everything is legal at 60, and the exceptions are exactly the `PRESERVE` items**, which only the running core can
name. A Manager reading the world database would decide wrongly in both directions.

## 4. How `Player::LoadFromDB` reacts

Observed on the cap-60 realm with real characters imported through the Manager (E1-E4), and read in `PlayerStorage.cpp`:

| State at load | Reaction | Evidence |
|---|---|---|
| level > cap | none for a real player (section 1) | code |
| equipped item whose runtime `RequiredLevel` > level | `CanEquipItem` returns `EQUIP_ERR_CANT_EQUIP_LEVEL_I`, the item is **removed from the slot, deleted from the inventory tables and sent by mail** ("There were problems with equipping item(s)") | `_LoadInventory` `6076-6110`, `CanEquipItem` `2427`; E2: a level-60 character with the runtime-80 Turban equipped lost it from slot 0 and got a `This item(s) have problems with equipping/storing in inventory` mail |
| the same item in a bag / backpack | loads normally (`CanStoreItem` has no level rule) | E2 (the Legwraps stayed in the backpack, no mail for them) |
| known spell above the working level | `_LoadSpells` calls `addSpell` blindly: **no level check, no removal** | `PlayerStorage.cpp:6643-6658`; E3: after a clamp to 60 the character still had all 1512 spells, 14 of them gated above 60 |
| stock talents | `character_talent` is empty in all real characters (0 rows in the 2.5M-spell realm); CoA talents are spells (section 5) | DB |
| glyph rows | `_LoadGlyphs` copies the rows with no level check; **0 rows exist** in the real realm | `Player.cpp:15835`, DB |
| active / rewarded quests | no level check in `_LoadQuestStatus*` (effective quest levels are compressed by the module) | code |
| pet | `Pet::LoadPetFromDB` -> `SynchronizeLevelWithOwner`: a hunter pet above its owner is lowered to the owner's level, a summon follows the owner (`Pet.cpp:2366`); `GivePetXP` gives nothing at or above the owner's level | code |
| class abilities (level-driven) | `OnPlayerLevelChanged` -> `AscensionClassService::SynchronizeProgression` (see section 5) | code, E3 |

So the *load itself* never holds anything back by level except the equip check, and that check **destroys the placement and mails the item**: holding must happen **before** load.

## 5. How CoA abilities, talents and builds are represented

CoA availability is **data driven by level, not `SpellInfo::SpellLevel`**:

* `AscensionProgression::Ranks` (2709 rows `{class, first spell of chain, spell, RequiredLevel}`), `AscensionCompatData::ClassSpells` (847), `LegacyGeneratedClassSpells` (270),
  `TaughtAbilities` (20), `TalentReplacements` (15) are compile-time tables; `CoATalentEntries` (entry id, class, spec, up to 3 rank spells, `AECost`, `TECost`,
  `RequiredLevel`), the automatic-entry dependencies and the **talent budgets by class and level** (`CoATalentBudgets {class, level, AE, TE}`) are read at startup from the client DBCs
  `CharacterAdvancement.dbc`, `CharacterAdvancementEssence.dbc`, `CharacterAdvancementClassTypes.dbc`, ... (`AscensionCoATalentData.cpp:114`).
* `SynchronizeProgression(player)` (called at level change, spec change, and on several commands) **removes** a `ClassSpells`/`LegacyGeneratedClassSpells` grant whose
  `RequiredLevel` exceeds the live level (`AscensionCompat.cpp:700-745`), and **learns** missing grants, ranks and automatic talent entries whose level is reached. It never removes a
  `Ranks` row's spell, a paid talent, or a spell outside the tables. Observed (E3): 14 of the 142 level-table spells of a real level-80 ranger need > 60 and **stay** after the level drops to 60.
* A paid talent (`AECost`/`TECost` > 0) is a spell in `character_spell` (rank n = the entry's n-th `SpellIds`); a pick requires `player->GetLevel() >= entry.RequiredLevel` and
  `spent + cost <= budget(class, level)` **only when it is chosen** (`SetTalentRank`, `AscensionCompat.cpp:1552-1590`); a spell already in the book is not re-checked against a *lower* level or budget.
* `AscensionCoATalentState::KnownEntries / KnownRank / Spent` are pure functions of a `HasSpell` predicate, so the core can evaluate a character's talent state **from a spell set**, without a `Player`.
* **Builds are stored in `character_settings` in three shapes** (all integers):
  * `core.ascension_slot.<n>`: `[1, class, spec, N, (entry, rank) * N, M, (button, action) * M, 0...]` (`SpecializationSlotRecord`, `ParseSpecializationSlot`);
    `core.ascension_slot.active` is the active slot;
  * `core.ascension_build.<spec>` and `core.ascension_slot.<n>.build.<spec>`: `[count, pick * count]` with `pick = entry * 10 + rank` (`StoreBuild`);
  * `core.ascension_bar.<spec>` and `core.ascension_slot.<n>.bar.<spec>`: `[count, (button, spell) * count]` (`StoreBar`).
  The **active slot's record is regenerated from the live spellbook at every save** (`SaveSlot` -> `StoreSlot(LiveSlot)`); an inactive slot's record is stored data and is applied by `SwitchSlot` through
  `ApplyKnownEntriesUpload`, which refuses entries above the level or over the budget. Observed (E2): after one login and save, `core.ascension_slot.0` had been rewritten from the live state.

## 6. The P0 hazard, stated against the code

The Phase 4/5 merge replaces a `character_settings` source as a whole (`cx.map("settings", ...)`). Under projection:

```text
C0  core.ascension_slot.0 = full level-80 record (83 entries, actions)
B0  core.ascension_slot.0 = what the level-60 realm regenerated at its first save (the projected subset)
B1  the same, after the player moved one talent at level 60
```
`B0 -> B1` differs, so the whole record would replace `C0`'s and **erase every suspended 61-80 entry**. Even with no edit at all, any save rewrites the record; the
generic map merge is not enough. The same holds for `core.ascension_build.*` and the bars (a held spell on a bar button), and for `character_action` (model `actions`).
All of them are decomposable (section 5): entries by id, picks by id, buttons by index. They are merged **semantically** under projection (overlay), and a record that does not parse is
**blocked** (kept canonical, not applied), never replaced.

## 7. Classification of portable state under projection

| State | Class | Rule (and why) |
|---|---|---|
| `progression.level`, `progression.xp` | **Projection-owned** | working = `(cap, 0)`; any `B0 -> B1` change is working-copy state and never reaches the canonical `(level, xp)` |
| money, honor, arena points, currencies, titles, explored zones | Apply | level independent; merged by delta as before |
| reputation, rewarded quests, client data, macros, wardrobe, collections, extensions | Apply | level independent |
| active quests | Apply | no level check at load; effective quest levels are compressed by the module |
| skills | Apply | rank caps follow the level at level-up; loading a higher rank does not fail |
| items: **equipped** (slots 0-18) or in a bag slot (19-22) with `runtime RequiredLevel > projected level` | **HoldByProjection** | the load would remove and mail them (section 4) |
| items: anywhere else (bags, bank-side containers, backpack) | Apply | loads normally; the realm refuses to equip them by its own rule |
| spells that appear in a CoA level table and whose **every** granting source needs `> projected level` | **HoldByProjection** | `CanPortableSpellExistAtProjectedLevel`; the core would not remove them by itself (section 5) |
| spells in no level table (racial, proficiencies, professions, mounts, stock, item-learned) | Apply | level independent |
| paid talents: entry `RequiredLevel > L`, then in data order those that exceed the AE (class tree) / TE (spec tree) budget of `L` | HoldByProjection | deterministic: keep the cheapest-level-first prefix that fits; the dependency closure of automatic entries is held with them |
| `core.ascension_slot.*`, `core.ascension_build.*`, `core.ascension_bar.*` | Apply (projected) + **overlay merge** | held entries/picks/actions are removed from P0 and survive in C0 (section 6) |
| a build record that does not parse | **Unsupported** | blocked: kept canonical, not applied, reported |
| `character_action` buttons whose action is a held spell | HoldByProjection | removed from P0, kept canonical |
| stock talents | Unsupported if any | none exist; a row would be refused rather than guessed |
| glyph rows | Apply | no level gate at load; none exist |
| pets | Apply | the core syncs the pet level to the owner at load (a normalisation, not progress) |
| item-linked appearance data | n/a | none (Phase 6) |

## 8. Policy that follows

1. **Activation.** `canonical.level > destination.max_player_level` -> projection active at `L = max_player_level`; otherwise none. Matrix: 80->80 none, 80->70 `L=70`, 80->60 `L=60`, 70->60 `L=60`, 60->60 none.
2. **Who decides.** The running core decides items, spells and builds (section 3: only it knows the post-scaling templates; section 5: only it holds the CoA tables) through one pure function, exposed as the console
   command `portable project` (a read-only query) and inside the online import job. The result is a **manifest**: held item ids, held spell ids, held `character_action` buttons and the projected build vectors, stamped with
   the core's `progression_signature` and the policy version. The Manager never evaluates level requirements itself.
3. **Stopped realm.** A projection needs a manifest. On a stopped realm the Manager cannot obtain one and **refuses explicitly** (`ProjectionNeedsRunningCore`) unless the caller supplies a manifest taken from a core with the realm's current signature. Nothing is guessed.
4. **P0 is an ordinary working copy.** `P0 = C0 - held + (level, 0) + projected builds`. Import, update, `B0`, `B1` and checkpoints work on it with the Phase 3/4/5 machinery; only the projection-owned parts of the merge are special (level/xp frozen, builds merged as an overlay). Held items map as `filtered`, held spells are absent from `B0`/`B1`, so the existing three-way merge keeps them without a second engine.
5. **Session pinning.** A session is pinned to `(content_profile_hash, progression_signature, projection policy version, projected level)` at the baseline. The pin travels in the (version 2) session messages and the Owner refuses a message under another pin. The signature is constant while a worldserver runs; a different one means a restart, which ends the session; the Host then does **not** re-arm the old working copy: it must be re-projected under the new profile first, and the realm refuses logins in the meantime (the session row stays `ended`).
6. **Profile change** (cap 60 -> 70): the next update re-projects with the new manifest; entries newly allowed at 70 are restored, entries still held until 80 stay canonical-only.
7. **Capabilities.** `RealmCapabilities` profile version 2 carries `progression { max_player_level, projection_protocol, progression_signature, scaling_enabled }`. The core's `progression_signature` is a SHA-256 over `MaxPlayerLevel`, the content-scaling switches that decide the layout, the policy version, a digest of the compile-time CoA progression tables and the talent data/budgets as loaded, and a digest of the effective `(entry, RequiredLevel)` of every item template (a few MB hashed once at startup, not the item database). It moves whenever anything above moves.

## 9. Experiments (cap-60 realm, scratch databases, bot logins = real `LoadFromDB`)

* **E1** level-80 ranger (real, 21 items, 1512 spells) logged in on cap 60: level read as 80 by `LoadFromDB`; only the bot manager lowered it ("clamped ... to server level cap 60"), after which `SynchronizeProgression` ran at level 60.
* **E2** level-60 necromancer with a runtime-80 turban equipped and a runtime-80 legwrap in the backpack: the turban left slot 0 and arrived by mail; the legwrap stayed; `core.ascension_slot.0` was rewritten at the first save.
* **E3** spell count 1512 before and after the drop to 60: no ability above 60 was removed (14 of them gated above 60 in the level tables, 5 above 70).
* **E4** `.coascale item` samples (section 3).

## 10. What is deliberately not done in v1

* Projection on a stopped realm without a manifest; any guess from the world database.
* A conflict-resolution UI, Registry, Relay, Wildcard, the personal bank, equipment sets.
* Holding items outside the equipment/bag slots, or holding by rules other than the level (skill, spell, proficiency requirements stay the realm's own business, as in Phase 4).
