# Portable Appearance & Collections (Phase 6)

Scope: CoA only. Wildcard transfer stays disabled. Not in this phase: Registry/Relay, Wildcard, the personal Ascension bank,
equipment sets, level-cap projection.

This document is the **audit first** (sections 1-4, written from the code of the core fork `feat/portable-session-bridge`
and its modules before any Phase 6 code existed), then the design (5-8) and the verification record (9).

## 1. Method

Ownership is taken from the code that reads and writes each table, not from the table name. Files traced:

* `src/server/coa/AscensionCompat.cpp` (`AscensionCollectionService`): the only reader/writer of the appearance, wardrobe,
  outfit and vanity tables. Handlers: `HandleApplyAppearances`, `HandleSetAppearanceVisibility`, `HandleSaveOutfit`,
  `HandleDeleteOutfit`, `CollectItem` / `CollectItemAppearance`, `DeliverVanityItem`, `LoadPlayerState`,
  `OnPlayerLogin`.
* `src/server/coa/AscensionCharacterSelection.cpp`: `character_ascension_state`, `account_ascension_settings`.
* `modules/mod-ascension/data/sql/db-characters/*.sql` and `pending_db_characters`: the DDL.
* Client data read at startup (`Data/dbc/Appearances.dbc`, `ItemAppearances.dbc`, `VanityCollection.dbc`, `ItemSet.dbc`).

## 2. What the code actually does

| Fact (file:symbol) | Consequence |
|---|---|
| Appearance ids are the ids of `Appearances.dbc` (client data, field 0). `ItemAppearances.dbc` maps item entry to appearance id; `VanityCollection.dbc` lists the vanity item entries. None of this is in a database. | Ids are **content ids of the CoA client data**: namespace `coa`, kinds `appearance` and `item`. They are stable between CoA realms that run the same client data. Woodworking appearances are synthesised at startup (`LoadWoodworkingAppearances`, appearance id = item entry) and are not in the DBC. |
| `account_appearance_collection(account_id, appearance_id, source_item)` is written only by `CollectItemAppearance` with `INSERT IGNORE`, and read by `LoadPlayerState`. Nothing ever deletes or updates a row. | Account scoped, **permanent and monotonic**. `source_item` is bookkeeping: the packet to the client takes the source item from the DBC (`SendAppearanceCollection`), never from this column. |
| Appearances are collected from the **items a character owns**: `ScanPlayerInventory` at login, `CollectItem` whenever an item is acquired. | Importing items re-derives some unlocks by itself; the union with the canonical set is still required (unlocks outlive the items). |
| `account_vanity_collection(account_id, item_id)` is written only by `CollectItem` (`INSERT IGNORE`, and only when `UnlockAllVanity` is off, or for a bank item) and read by `LoadPlayerState`. | Account scoped, permanent, monotonic. |
| Four item entries are **bank vanity items** (`BankVanityItems = 110000, 134985, 509892, 1180097`); owning one runs `LearnOwnedBankSpells`. | They open the personal Ascension bank, which is out of scope: excluded from the portable vanity collection (Deferred). |
| `character_appearance(guid, category_id, appearance_id)`: the **selected** appearance per category (1-14 equipment slots, ammunition, 56-58 cosmetic spells, 55 item set, up to 68). `SaveActiveAppearances` does `DELETE` + `INSERT` in one transaction on every apply; `HandleApplyAppearances` first requires every selected id to be in the account collection. | Character scoped, **mutable selection**. Written when the player applies, not by `SaveToDB`: the database row is always current. |
| `character_appearance_settings(guid, can_see_item, can_see_spell)`: `REPLACE` by `HandleSetAppearanceVisibility`. | Character scoped, mutable setting. |
| `character_appearance_outfit(guid, name, appearances)`: saved named outfits (at most 100, name at most 64 bytes, a space separated list of appearance ids, each must be collected). | Character scoped, mutable, purely cosmetic. Not the equipment manager: `character_equipmentsets` stays Deferred. |
| `RefreshCosmetics` re-derives the cosmetic auras from the selection at every login and apply. | Cosmetic auras are **derived**; `character_aura` stays CharacterLocal. |
| No table links an appearance to an `item_instance`. `item_instance` has no transmog column; `characters.equipmentCache` is a visual cache the server rebuilds. | The selection never touches a gameplay item, and reconciling it **cannot create or delete an item**. |
| `QueueOwnedCompanionSpells` / `LearnOwnedBankSpells` turn owned vanity items into mount and companion **spells** at login. | Mounts and cosmetic pets have no table of their own: they are `character_spell` rows (already carried since Phase 2) plus the account vanity ownership above. |
| Realm configuration: `CoA.UnlockLocalAppearanceCatalog` puts the **whole catalogue** into the in-memory collection at login, `CoA.UnlockAllVanity` makes the vanity collection virtual (nothing is stored). `CoA.AutoCollectAppearances` controls the item scan. | On a realm with those switches on, the collection tables hold (almost) nothing and there is nothing to carry; unlocks are meaningful on realms that unlock progressively. Both smoke realms of earlier phases had the switches on, the Phase 6 gate realms have them off. |
| The collection state of a player is loaded **once at login** (`LoginState`/`LoadCollectionState`) and cached; the core only ever `INSERT IGNORE`s into the account tables. | Adding account rows with `INSERT IGNORE` while the account is online is safe (they show up at the next login, never lost). Character appearance rows must only be written while the character is offline or being imported. |

## 3. Classification

`PortableAppearance` is the character section that travels with the character snapshot. `PortableCollection` is the
profile-wide permanent unlock set held by the Owner. `RealmLocal` is never carried. `Deferred` is portable later by decision.

| Table / field | Scope | Behaviour | Class |
|---|---|---|---|
| `character_appearance.category_id -> appearance_id` | character | mutable selection | **PortableAppearance** (`appearance.active`) |
| `character_appearance_settings.can_see_item, can_see_spell` | character | mutable setting | **PortableAppearance** (`appearance.can_see_item/spell`) |
| `character_appearance_outfit.name, appearances` | character | mutable, cosmetic | **PortableAppearance** (`appearance.outfits`) |
| `account_appearance_collection.appearance_id` | account | permanent, monotonic | **PortableCollection** `coa:appearance` |
| `account_appearance_collection.source_item` | account | bookkeeping only | not carried (rewritten as 0 on the destination) |
| `account_vanity_collection.item_id` (not a bank item) | account | permanent, monotonic | **PortableCollection** `coa:vanity` |
| `account_vanity_collection.item_id` in `{110000, 134985, 509892, 1180097}` | account | opens the personal bank | **Deferred** (never written, never read into the collection) |
| mounts and cosmetic pets (`character_spell` rows) | character | already carried as spells | unchanged (Phase 2), no new class |
| `character_aura` (cosmetic auras) | character | derived from the selection at login | RealmLocal |
| `character_ascension_state.active` | character | char-select UI flag of that realm | RealmLocal |
| `account_ascension_settings.sort_order` | account | opaque client char-select order of that realm's guids | RealmLocal |
| `character_equipmentsets` | character | equipment manager | Deferred (explicitly out of scope) |
| `mod_ascension_bank_item/log/money/tab` | account / character | personal bank | Deferred (explicitly out of scope) |
| `coa_account_warchest` | account | one-time reward claim | RealmLocal (a gameplay reward, not a cosmetic) |
| `coa_wildcard_skill_card*`, `coa_wildcard_specialization_cache` | account | Wildcard | Deferred (Wildcard transfer is disabled) |
| `account_data`, `account_tutorial`, `account_instance_times` | account | client/UI/instance state | RealmLocal |
| `item_instance`, `character_inventory` | character | gameplay items | unchanged: appearance logic never reads or writes them |
| `Appearances.dbc`, `ItemAppearances.dbc`, `VanityCollection.dbc` | content | the id catalogue | content, not state: defines what a destination **knows** |

The table registry (`realm/registry.rs`) gets a new class `Appearance` for the three character tables (they were
`Deferred`); the two account tables stay `Collection`.

## 4. Findings that shape the design

1. **Knowledge is a property of the destination's client data**, not of its database. A destination knows an appearance id
   iff it is in its `Appearances.dbc` (and a vanity item iff it is in its `VanityCollection.dbc`). The Manager reads those
   two files of the target realm (`realm/knowledge.rs`). An id the destination does not know is **kept canonically and is
   never written** to the destination, and is written when the character reaches a realm that does know it.
2. **Collections never ride in a character checkpoint.** They are a separate change-detected channel (section 6).
3. The collection write is `INSERT IGNORE` only, exactly like the core's own writes, so it is safe on a running realm.
4. The character appearance write is part of the character import (online job, offline plan) and of the in-place update,
   both of which already require the character to be offline.
5. Woodworking appearances (synthesised ids) are not in the DBC: the Manager treats them as unknown to a destination, so
   they stay canonical and are not applied. This is a documented limitation (they are crafted-item cosmetics).
