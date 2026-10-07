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

## 5. The selected appearance in the character (`PortableCharacter::wardrobe`)

```text
wardrobe: { active: { "<category>": <appearance id> }, can_see_item, can_see_spell, outfits: { "<name>": [<appearance id>...] } }
```

* Absent when empty (`skip_serializing_if`), so every snapshot made before Phase 6 keeps exactly its bytes and its hash; there
  is no format-version bump (`format_version` stays 1: the field is optional and `deny_unknown_fields` still refuses anything
  else).
* Ids are plain numbers in the character's `content_namespace` (`coa`), like spells and quests. The namespaced forms
  `coa:appearance:<id>` / `coa:item:<id>` are used where an id leaves the character: the collection kinds.
* **Export** (`script.rs`, `export.rs`): three fixed `SELECT`s, only where the three tables exist. Rows the realm itself would
  ignore (a category outside 1..68, an appearance of 0) are left out, so the model is what the realm shows.
* **Merge** (`merge.rs`): the same keyed-map / scalar rules as `settings`. In a session `merge3(C0, B0, B1)` only the
  `B0 -> B1` choices are progress; what the realm never showed stays in the canonical character through any number of
  sessions. In an update `merge3(realm now, synced, canonical)` reports a conflict only where both sides changed the same
  category differently.
* **Import / update** (`wardrobe.rs`, `plan.rs`, `update.rs`): written only with the knowledge of the destination
  (section 4.1). `writable()` is the part of the wanted state the realm can hold plus whatever it **already shows** (an id the
  realm put there itself is never deleted because the Manager's table does not list it). An outfit that mentions an unknown id
  is held back whole. The update replaces the three tables of the character as a whole when what the realm can hold differs
  from what it shows, in the same transaction as the rest of the update, counted in `keyed_rows`. No `item_instance` or
  `character_inventory` statement is ever produced by the appearance code (a test asserts the item counts of the script).
* **Online import** (`online.rs`): `job_model()` carries the `wardrobe` restricted to what the destination knows (none without
  knowledge). The core's job parser accepts the section with the same bounds as the model (section 8).
* **Registry**: `character_appearance`, `character_appearance_outfit`, `character_appearance_settings` are the new table class
  `Appearance` (optional-module tables: they are not required to exist, unlike the `Portable` ones).

## 6. Account collections (`coa:appearance`, `coa:vanity`)

They belong to the Owner's profile, not to a character: `collection(profile, kind) = revision + compact sorted id set + hash`
(the Phase 1 design, unchanged: delta + LEB128 varint, about one byte per id for a dense wardrobe). `canonical = canonical U
incoming`; a realm that lacks an unlock never removes it.

```text
Host                                              Owner
 fingerprint(account, kind)  ----(cheap)---->     (nothing)
   changed?  read ids, hash
   hash != last acknowledged?
 CollectionObserved{server, kind, set} -------->  union into the profile collection
                                       <--------  CollectionAck{outcome, revision, hash, canonical?}
 canonical present (it has ids the realm lacked)?
   apply the known ones to the account (INSERT IGNORE)
```

* **Change detection** is a fingerprint of the realm rows (count, max, sum, checksum): one aggregate query. The set is read and
  sent only when it differs from the last one the Owner acknowledged; an unchanged account costs one query per kind every
  `collection_interval_secs` (default 300) and nothing else. A session start and a final checkpoint always look. The first look
  of an account reports even an empty set: that is how the Owner learns of the realm and answers with what it holds.
* **Never in a checkpoint.** A `PortableCheckpoint` carries the character only (a test asserts that no checkpoint message
  mentions a collection). The 60-second cadence never touches the collections.
* **Transport-neutral.** `CollectionObserved`, `CollectionState`, `CollectionAck` are canonical JSON with
  `protocol_version = 1`, `deny_unknown_fields`, a size cap checked before parsing (`MAX_COLLECTION_MESSAGE_BYTES`), the set as
  base64 of the compact encoding, its hash and its count. The Owner re-derives hash and count from the decoded set and
  answers `Rejected` (changing nothing) on any mismatch, an unknown kind, or garbage; a malformed message is an error and
  changes nothing.
* **Idempotent and restart-safe.** A lost acknowledgement re-sends the same set; the union adds nothing and no second revision
  is made. The Host keeps the pending message and the last acknowledged hash in `host_collection` (schema 6), so a restart
  neither loses a message nor reads or sends what was acknowledged.
* **Owner / Host separation.** `OwnerService` is the only code that touches the canonical collection; the Host knows its own
  `host_collection` rows and the realm. The Host never opens the Owner's SQLite file.
* **Writing to a realm** (`realm/collections.rs`): the ids the destination knows and does not hold yet, `INSERT IGNORE`, in
  chunks of 2000 rows in one transaction, guarded by "the account exists". It is safe on a running realm for the same reason
  the core's own writes are: the core loads an account's collection once at login and never deletes or updates a row. The bank
  vanity items are never read nor written. `source_item` is written as 0 (the core never reads it back).
* **A realm that learns more later** (a newer client data directory) is brought up to date by applying the canonical state
  again; only the new known ids are written.

## 7. Where the destination's knowledge comes from

`ImportOptions::knowledge` is an `Arc<RealmKnowledge>` read from `<Data>/dbc/Appearances.dbc` and `VanityCollection.dbc` of the
destination realm (`RealmKnowledge::from_data_dir`, a strict WDBC reader with a size/header/record check). Without it the Manager
writes **no** appearance and **no** collection to the realm and reports `wardrobe` as not applied: everything stays canonical.
The harness takes it as `--data-dir`.

## 8. Core changes (`feat/portable-session-bridge`)

Only the online import job needed the core: the job format gained the optional `wardrobe` object (same shape as the model),
parsed by the strict `CoAPortableJson` reader with the model's bounds (68 categories, 100 outfits, 64-byte names, 69 ids per
outfit), written with three new prepared statements (`CHAR_INS_PORTABLE_APPEARANCE`, `_APPEARANCE_SETTINGS`,
`_APPEARANCE_OUTFIT`, `CONNECTION_BOTH`) inside the existing import transaction; the result file reports the number of rows as
`wardrobe`. Nothing of the session gate, the checkpoint or the release operations was touched. The collections need no core
change: they are written by the Manager as `INSERT IGNORE`.

## 9. Verification (Phase 6 gate)

| What | Result |
|---|---|
| `cargo test --workspace --locked` | 436 passed (413 before the phase), 39 live tests ignored without a database |
| Live tests, two real MySQL servers (A, B) | 39 passed: the whole earlier suite (now with a wardrobe on the fixture character 1002) plus 5 new ones in `live_wardrobe.rs` |
| Real worldserver (new core build), 3 tests | passed: automatic `B0` and checkpoints, the gate, and the **online import including the wardrobe** (read back equal to what the offline importer writes) |
| `node tools/check-i18n.mjs`, `npm run build` | green |
| Large collections, simulated realm | 10 000 and 50 000 scattered ids: compact message under 4 bytes per id (about 1 per id when dense), unchanged collection costs one fingerprint and **zero** reads, empty-realm arrival gets the whole set once |
| Large collections, real MySQL | 10 000 ids: write 0.36 s, fingerprint 0.14 s, full read 0.08 s; 50 000 ids: write 0.75 s, fingerprint 0.08 s, full read 0.10 s, second apply writes 0 rows, no duplicates, compact set 50 KB |

What the tests prove, by property:

* a checkpoint never carries a collection; a 60-second cadence never reads one;
* union only: a realm with fewer unlocks never removes one, a realm's own row is never removed by an apply;
* an id the destination does not know is **not written** and stays canonical (selection, outfit, collection), through sessions and
  updates; a destination that knows more gets the rest on the next apply;
* reconciling an appearance changes no item: item sets are identical before and after (merge, simulated session, update script,
  and the real database round trip);
* a lost acknowledgement, a duplicate, a restart of the Host between observe and acknowledge, hostile or oversized or
  mismatching messages, a kind that is not carried: nothing is merged twice or at all;
* the bank vanity items are never read from or written to a realm.

### The real-client gate (disposable pair, the owner's client)

Realm B "PT Guest" and realm A "PT Home" with `CoA.UnlockLocalAppearanceCatalog = 0` and `CoA.UnlockAllVanity = 0`, so unlocks
come only from play. The Host ran against B with a 20 s checkpoint and a 30 s collection look.

1. On B: `.additem` of two weapons, a helm, a vanity mount and a vanity pet; their appearances applied (head and weapon); the
   added items then destroyed. Result in the Owner after logout: 16 appearances and 7 vanity items in the profile collections
   (revisions 5 and 3), the selection `{1: Lionheart helm, 14: Thunderfury}` in the character, final checkpoint acknowledged.
2. Back on A: the character in place from canonical, the collections applied by union (16 appearance rows, 7 vanity rows, no
   duplicates, nothing lost), the one synthetic id unknown to A kept in the Owner and **not** written to A.
3. Logged in on A with the real client: the wardrobe unlocks, the vanity unlocks, the selected transmog and the original gameplay
   items were all there (owner's check, 2026-10-07).

## 10. Limits and findings

* **Realm switches decide whether there is anything to carry.** The repack ships `UnlockLocalAppearanceCatalog = 1` and
  `UnlockAllVanity = 1`; there the collection tables hold nothing (the core keeps the unlock-all in memory) and a wardrobe sync is
  empty by design. It is meaningful on realms that unlock progressively.
* **The destination's client data must be known.** `ImportOptions::knowledge` is read from `Data/dbc`; the Manager integration has
  to supply the realm's data directory. Without it nothing is written (and `wardrobe` is reported as not applied).
* **A core without the Phase 6 job parser refuses a job that carries a wardrobe** (`wardrobe` is an unknown field to it). There
  is no version negotiation yet; the online import needs the Phase 6 core build when the character has a wardrobe.
* **Woodworking appearances** (synthesised ids that are not in `Appearances.dbc`) are treated as unknown by a destination: kept,
  never applied.
* **An id a realm learns later** is written by the next collection apply, but a selection that was held back is not retroactively
  re-applied by an update (it is "left alone" because canonical did not change since the last sync).
* **A realm copy that diverged without a session cannot be updated in place** (`UpdateConflicts`, by design, nothing written). In the
  gate the old A copy had been played unmanaged since Phase 4 and had to be re-imported. A conflict-resolution policy is a later decision.
* The collection look is polled by the Host (`collection_interval_secs`); the Host driver is still a library plus the harness
  (`host-run`), not yet part of the Manager application.
