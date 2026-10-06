# Portable export from a realm (Phase 2)

Code: `crates/coa-core/src/portable/realm/`. Scope decisions: `PORTABLE_CHARACTERS_AUDIT.md` sections 15-16.

Phase 2 is **read-only**. It reads one **offline** character out of a realm's characters database into a
`PortableCharacter` and (for "Make Portable") registers it in the local store. It never writes to a realm.

## What it does

| Function | Meaning |
|---|---|
| `inspect_characters(db)` | "Inspect realm characters": every character of a non-bot account with the reasons it cannot be exported yet |
| `export_character(db, guid, character_id, prior_items)` | "Export snapshot": the portable model + item observations + warnings |
| `make_portable(db, store, profile, server_id, guid)` | "Make Portable": export, then register in the local store in one transaction (revision 1, bound to the realm, items mapped) |

No Tauri command or UI exists yet (Phase 9).

## How the realm is read

* Through the Manager's existing `Db` abstraction (the bundled `mysql` client, or the same client inside a Docker
  database container). No driver, no new connection type, no published port. The Wildcard realm profile is routed by
  `Db` exactly like everything else, and **the database decides the ruleset**, not the caller.
* **Fixed SQL, typed input.** Every statement is a literal in `script.rs`. The only input is the character's local
  guid as a `u32` formatted as a number. Table and column names of *unknown* tables come from `information_schema`,
  are checked as plain identifiers and appear only in `SELECT COUNT(*)`.
* **One consistent read-only snapshot.** All statements for a character run in one `mysql` session inside
  `SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ; START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY;`.
  Verified against a real server: a write inside the script fails, and a concurrent write by someone else in the
  middle of the script is not seen.
* **Hex-encoded text.** Names, item texts, settings and macro blobs are returned as `x<hex>`, so tabs, newlines, quotes
  and backslashes cannot confuse the parser.
* Writing into a realm (Phase 3) must use prepared statements or the realm's own in-core import; this read path is not
  a basis for it.

## Who may be exported (`Blocker`)

| Blocker | Rule |
|---|---|
| `Online` | `characters.online <> 0` (also after a crash, when the flag is stale: stop the realm or log in/out) |
| `Deleted` | `deleteDate` is set |
| `BotAccount` | account `COABOT*` or `COAMANAGER` |
| `ActiveChallenge` | a row in `coa_character_challenge` |
| `ActiveGameMode` | `coa_character_gamemode.gameMode <> 0` |
| `ActiveCustomTrial` | a row in `coa_custom_trial_active` |
| `PendingManastormCaches` | a row in `ascension_manastorm_cache` (it names item guids that would be orphaned) |
| `UnclassifiedState(table)` | a table this Manager does not know has a per-character column with rows for this character |

Finished challenges, game mode 0 and condition flags are history, not active state, and do not block (decision D6:
*active* state blocks). A realm that lacks a core table (`SchemaMismatch`) cannot be exported from.

## What is exported (v1)

17 tables: `characters`, `character_inventory`, `item_instance`, `character_gifts`, `character_spell`,
`character_talent`, `character_skills`, `character_glyphs`, `character_reputation`, `character_queststatus`,
`character_queststatus_rewarded` (`active = 1`, as the realm loads it), `character_action`, `character_pet`, `pet_spell`,
`character_pet_declinedname`, `character_settings` (by policy), `character_account_data` (macros, type 5).

**Not exported by decision** (deferred): collections, the personal Ascension bank, equipment sets, per-character wardrobe
(`character_appearance*`), `mod_craftsmans_codex`. Auras, cooldowns, mail, guild, social, achievements, daily/weekly
quests, position and homebind are never part of v1.

Left out *with a warning* (the realm would delete them on load anyway): inventory rows without an `item_instance`,
items in a bag that is not on the character, items in buyback slots.

### `character_settings` policy (`policy.rs`, version 1)

* **Carry**: known CoA gameplay state (`core.ascension_*`, `core.spell_charge.*`, `core.destiny_weaver`, ...), and
  `core.wildcard*` on a Wildcard realm.
* **Drop**: `coa.bot*`, `coa.gameplay_test`, `coa.highrisk`.
* **Quarantine**: everything else is kept in the snapshot as the opaque extension `coa:unlisted-settings` (never
  destroyed, never applied at a realm).

### Table registry (`registry.rs`)

All 163 tables of the characters schema are classified (`Portable`, `Deferred`, `Collection`, `AccountLocal`,
`CharacterLocal`, `Blocking`, `Realm`). A test fails if the real schema has a table the registry does not know, and at
export time an unknown table that holds this character's rows blocks the export. A silent half-export is not possible.

## Item identity

Realms recycle local item guids. `ItemObservation.identity` hashes what cannot change during an item's life (entry,
random property, crafter). A re-export keeps an item's portable id only if the identity behind the local guid is
unchanged; a recycled guid produces a new portable item. The store side is in `PORTABLE_FORMAT.md` (item mapping
lifecycle).

## Tests

* `realm/tests.rs`: on **recorded answers of a real MySQL** (`testdata/golden/`), no database needed: naked level 1,
  geared level 80 (bags, enchants, gems, random suffix, crafted-by, texts, charges, gift, currency tokens), quests,
  reputation, action bars, 300 spells + talents + glyphs + skills + CoA settings + binary macros, hunter and summon
  pets with declined names, every blocker, item id stability and recycling, Wildcard ruleset, CRLF, 400 damaged
  answers (no panics), registration in the store (one transaction, refused twice, rolled back on failure).
* `realm/live.rs` (`#[ignore]`): the same answers re-recorded/verified against a **real disposable MySQL**, plus the
  read-only and snapshot-consistency proofs, the 21 genuine template characters of the schema fixture, and the proof that
  `make_portable` changes nothing in the realm. Instructions are at the top of the file.
* `tools/gen_portable_fixture.py` writes `testdata/realm-fixture.sql` (deterministic).
