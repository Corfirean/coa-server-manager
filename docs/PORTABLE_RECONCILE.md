# Round trips: reconciling a session and updating a realm character in place (Phase 4)

Code: `crates/coa-core/src/portable/{merge.rs, realm/{reconcile,update}.rs, store/{sync,journal}.rs}`,
migration `migrations/004_reconciliation.sql`. CoA only. **Wildcard portable transfer is not supported**: every Wildcard
setting stays quarantined until a separate key-by-key audit, and CoA <-> Wildcard transfers are refused.

Realms are working copies; the Manager's canonical store owns the character. A realm changes a character on its own the
moment it loads it (default spells, honor resets, items it cannot place, ...). Phase 4 makes sure that **only what the
player did** travels back, and that **only the portable subset** is written into a realm that has the character already.

## The three-way model

```text
C0  canonical when the character joined the realm   (= the snapshot the realm was last synced with)
B0  the realm's own first load/save of it           (normalisation done, nothing played yet)
B1  the realm now / at the checkpoint

canonical' = merge3(target = C0, base = B0, ours = B1)      only B0 -> B1 is progress
realm'     = merge3(target = realm now, base = synced snapshot, ours = canonical')    in place
```

`merge3(target, base, ours)` applies **what changed between `base` and `ours`** onto `target`. Differences that exist between
`target` and `base` already (the realm's normalisation, or in the other direction the realm's own additions) are *left
exactly as they are*: automatic destination normalisation can never overwrite canonical state.

| kind | rule |
|---|---|
| scalars | `ours` wins where `base != ours` |
| accumulators (money, honor totals, kills, arena points, xp inside a level) | the **delta** `ours - base` is added, clamped |
| bit sets (currencies, titles, explored zones, taxi nodes) | bits gained are added, bits lost removed |
| keyed collections (spells, skills, reputation, quests, action buttons, carried settings) | keys added / removed / changed in `ours` are applied |
| items, pets | matched by **portable id**, see below |
| stock talents, extensions (quarantined settings) | never touched |

A **session** reconcile is lenient (`ours` wins); an **in-place update** is strict: if the realm and the canonical character both
moved the same thing away from the synced snapshot, and disagree, the update is refused with the list of conflicts and
**nothing is written**. Accumulators never conflict (both deltas are added).

### Realm-filtered state is not deletion

Per realm and per item/pet the store keeps a mapping with a **presence**:

| presence | meaning | at reconcile |
|---|---|---|
| `present` | canonical owns it, the realm shows it | absent in B1 and present in B0 = the player got rid of it: removed, mapping retired |
| `filtered` | canonical owns it, the realm does **not** show it (held back / mailed before B0) | **kept**; the mapping is *not* retired; if it shows up again it is taken |
| `realm_local` | the realm shows it, canonical does not own it (starter items, realm grants) | never merged, never written back |

So an item the realm moved out of the inventory before `B0` stays canonical, and an item the player destroyed after `B0` does
not. Item identity is `entry + random property` (the crafter is excluded); pet identity is `entry + pet type + summoning
spell`. A recycled local guid / pet number never inherits an old portable id (`guid_reused`).

## What is persisted

* `item_mapping.presence`, `pet_mapping` (stable per-realm pet ids with the same lifecycle as item mappings);
* `character_server_mapping.synced_hash/synced_payload`: the canonical snapshot a realm was last brought to;
* `realm_baseline`: `C0` and `B0` of the open session (one open session per character and realm), the session's `head_revision`;
* `import_journal` kind `update`, with the items/pets the update adds and retires.

## Session flow

1. `begin_session` (**after the realm's first load/save and before the character is played**): exports the realm's character
   as `B0`, classifies filtered / realm-local items and pets, persists `C0 + B0`. The join flow must own that ordering: a
   baseline taken later would count the progress made so far as the realm's own normalisation.
2. The player plays; realm offline or the character offline.
3. `reconcile_session(close = false)` = checkpoint, `(close = true)` = end of session. Every call applies the **whole**
   `B0 -> B1` delta to `C0` (not to the previous checkpoint), so repeating it never counts progress twice. A session that
   changed nothing makes no new revision. The canonical character must still be the session's own last output; if another
   realm moved it in between the call is refused (`StaleRevision`) and nothing is committed.

## In-place update

`update_realm_character`: reads the realm's character (`A_now`), merges `(A_now, synced, canonical, strict)`, preflights
(realm stopped, character exists, new item/creature entries known to the realm), journals, runs one MySQL transaction, records
the mappings and the new synced snapshot in one local transaction.

* the **same local character** is updated: guid, account, position, homebind, auras, cooldowns, instance saves and every row
  the model does not carry are never touched; nothing is deleted or re-created;
* only the **columns and rows that differ** are written: the script is a delta (`UPDATE` of changed columns, `DELETE` +
  `INSERT` of changed keyed rows, items/pets addressed by their mapped local ids; new items/pets get ids allocated inside the
  transaction above everything that names an id);
* moved items leave their inventory places before anything takes them (a swap cannot collide);
* items the realm filtered are not resurrected; items/pets the realm added stay;
* race, class and gender cannot change in place (refused);
* the transaction asserts its own result (item/inventory/pet counts relative to the start, no new orphan inventory rows) and
  fails whole, leaving the realm untouched;
* a session open on that realm blocks the update (`SessionOpen`); a running realm and an online character block it too.

### Crash recovery

Like an import, an update is journaled `prepared` before the realm transaction and carries a marker row
`coa.portable.import = <nonce> <revision>` plus `coa.portable.alloc = <item_base> <pet_base>` written **inside** the realm
transaction. `recover_imports` finds the marker: present = committed (mappings, synced snapshot and journal are finished from
the allocation row, after verifying the new items/pets exist), absent = never committed (aborted). Inconsistencies are flagged
`needs_attention`, never guessed.

## What was verified

Unit (`merge/tests.rs`, `store/tests.rs`, `realm/update_tests.rs`): normalisation alone changes nothing; only the session delta is
applied; repeating is idempotent; money delta; level-up; bit sets; filtered vs deleted items; reappearing items; slot
collisions; pet identity; extensions untouched; 300 rounds of random normalisation; persistence of baselines, presence, pet
mappings and the update journal; the generated update script (only changed columns, never position/homebind/`characters`
insert or delete, hex-only hostile text, deletes before inserts).

Live (`realm/live_update.rs`, two disposable MySQL servers): in-place update keeps all local state and touches only the tables
the change is about; an injected failure rolls everything back; lost answer after commit is recovered, a never-run update is
aborted; running realm / online character / open session / divergence are refused with nothing written; filtered items
stay canonical while deleted ones go, checkpoints never count twice, the realm's own items survive updates; pets keep ids
across sessions and recycled numbers do not inherit them.

Real worldservers (`realm/live_roundtrip.rs`, driven stage by stage): CoA characters from the schema fixture
A -> portable -> B, real load/save (`botcmd spawnbot` + `saveall`), `B0`, play (SQL with real ids plus a real `character level`
GM command), real load/save, reconcile, update of the original characters of A in place, real load/save on A, play on A,
reconcile, update of B in place, real load/save on B. Independent expectations were computed from the three snapshots
(not from the merge engine) and asserted for level, money, items and spells; local state before/after every in-place update
was compared.

Findings from the real servers: a real CoA worldserver changed a freshly imported fixture character substantially on its own
(55 of 60 items held back, synthetic spells deleted, default spells/skills/reputation/taxi added, honor and title reset);
none of that reached the canonical character. The fixture's synthetic `character_talent` rows make a real worldserver
assert (`Player::_LoadTalents`) for that one character on its **source** realm; this is a property of the fixture, not of the
update (the importer and updater never write `character_talent`). Items a realm grants **after** `B0` (a class kit
re-granted at a level change, loot) are indistinguishable from earned items and are carried; only what existed at `B0` is
normalisation.

## Manual real-client smoke test

`cargo run -p coa-core --example portable_smoke -- ...` (see the header of `examples/portable_smoke.rs`) performs one
operation per call against any database reachable through the MySQL client tools, with a Manager store in a directory of
your choice (`COA_DB_PASSWORD` in the environment). Use **copies**, never the live realm:

1. Make a disposable copy of the realm database (or of the whole install) that contains a real CoA character; run a second
   disposable realm (an empty install works, it needs a game account).
2. `list` -> `make-portable <realm-1> <guid>` -> note the printed character id. Stop both realms.
3. `import <id> <realm-2> <account>`; start realm 2; log in with the real client; look at the character; log out; stop it.
4. `begin-session <id> <realm-2>`; start realm 2; play for a few minutes (earn money, change gear, drop or sell an item, pick
   one up, learn something, maybe level); log out; stop it.
5. `reconcile <id> <realm-2> --close`; read the printed changes; `update <id> <realm-1>` (stopped realm); start realm 1 and
   log in with the real client: the character must be the original one with what was played on realm 2.
6. Optionally play on realm 1 and `begin-session`/`reconcile`/`update <id> <realm-2>` back: it must be the same local
   character on realm 2 at the same place.

### Result of the manual smoke test (2026-10-06, real Project Descension client)

Two disposable copies of a real CoA install (`coa-fixture`, real data and characters), character Shaniel (level 80, race 4 /
class 21):

1. Realm 1 -> portable -> imported into realm 2 (new local character 2527); real login, baseline `B0` captured (nothing held
   back, nothing added by the realm).
2. Played on realm 2: killed a boar and looted meat (stack 5 -> 6), deleted an item, accepted a quest, delivered a mount from
   the vanity collection. `reconcile`: canonical revision 2 carried exactly those changes (stack, removed item, quest, two new
   items) and nothing the realm did by itself.
3. `update` of realm 1 **in place** (same local character 2): items 33 -> 34, position and everything local unchanged. The
   owner logged in: the looted meat, the deleted item, the quest and the mount were there. The character was a ghost with
   a corpse in the Deadmines and stood in Orgrimmar: that is realm 1's own pre-existing state (verified in the database before
   and after), which an in-place update must keep.
4. Second round: baseline on realm 1, played (new item, two items moved), `reconcile` (revision 3), `update` of realm 2 in
   place: the new item and the moved items arrived, position unchanged.

Findings:
* **Order matters.** The baseline has to be taken after the realm's first load and *before* the character is played. After the
  first in-place update on realm 1 it was not, so a helmet delivered in that first session counted as the realm's own item
  and did not travel. This is the documented rule, but it is easy to break by hand: the Manager's join/return flow must take the
  baseline itself, not leave it to the user.
* **Transmog and the vanity collection are not carried.** `character_appearance`, `character_appearance_settings` and the
  account collections belong to the "per-character wardrobe extras / collections" deferred in Phase 2. An equipped item travels;
  its transmog appearance does not. Needs its own phase (what is character state and what is account state first).
* **Setup facts** (for the next person): the client's saved `realmName` must equal the realm's name or it loops on
  `REALM_NOT_FOUND`; a race/class needs a `playercreateinfo` row to be imported (race 3 / class 23 has none on this data); stale
  `online` flags and a realm flag of 3 block imports and the authserver respectively.
