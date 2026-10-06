# PortableImportService (production importer): design

Status: **implemented in Phase 5** (core fork branch `feat/portable-session-bridge`: `src/server/coa/CoAPortableImport.{h,cpp}`,
`CoAPortableJson.{h,cpp}`; Manager side `portable/realm/online.rs`). Differences from this design are listed at the end. It is a core-side
workstream next to `PlayerDump`. The offline importer ([`PORTABLE_IMPORT.md`](PORTABLE_IMPORT.md)) stays the tool for stopped realms and for
tests.

## Why in-core

A running worldserver owns the id counters, the character name cache and the realm character count in memory
(`ObjectMgr::SetHighestGuids`, `CharacterCache`, `World::UpdateRealmCharCount`). Anything written from outside collides with
ids the server is about to hand out and is invisible to name lookups until a restart. `PlayerDumpReader::LoadDump`
(`PlayerDump.cpp:764`) already solves this inside the server; the service follows its runtime invariants and adds the CoA
tables and the portable model.

## Trigger: an opaque job id, never a payload

```text
Manager                                              worldserver
  1. writes   <JobDir>/<job_id>.job   (atomically)
  2. RA: `.portable import <job_id>`  ------------->   validates job_id, reads ONLY that file from the fixed JobDir
                                                       runs the import, writes <JobDir>/<job_id>.result (atomically)
  3. RA reply: "OK <job_id>" / "ERR <code>" <--------
  4. reads the .result file, deletes both files
```

* `PortableImport.JobDir` is a worldserver setting (default `<realm>/PortableImport/`). The command takes **only** `job_id`, a
  UUIDv7 string matched against `^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`; the file name is
  built by the service as `<JobDir>/<job_id>.job`. No path, no separator, no payload, no SQL travels through RA, and RA's
  typed-operation rule in `ra.rs` is kept (one new typed operation, `import_job(job_id)`).
* Console only, with its own RBAC permission; one job at a time; the reply never contains character data.
* The job file is the snapshot envelope of `PORTABLE_FORMAT.md` (compressed canonical JSON + SHA-256) plus a small header:
  `{job_id, character_id, revision, account, nonce, expected_content_hash, ruleset}`. Size limits and the decompression cap are
  enforced **before** parsing; the decoder is strict (unknown fields refused, bounded lists) and validates the same limits as
  `PortableCharacter::validate`.

## The runtime invariants (taken from `PlayerDumpReader`)

All of these happen **inside the server, in one `CharacterDatabaseTransaction`**, on the world thread's DB path:

1. Refuse when the account does not exist, is a bot/Manager account, is full (`CharactersPerAccount`), or a character with this
   marker already exists (idempotency, below).
2. Allocate ids from the **live generators**: `GetGenerator<HighGuid::Player>().GetNextAfterMaxUsed()`, the item generator,
   `_hiPetNumber`; remap every reference through those maps (the same remap rules as the offline importer).
3. Name check with `normalizePlayerName` / `CheckPlayerName` and the name cache; a taken or invalid name gets the temporary
   name and `AT_LOGIN_RENAME` (as `.pdump load`).
4. Validate against the realm's *own* data, which the offline importer cannot see: `sObjectMgr->GetItemTemplate`, creature
   templates, `sSpellMgr`, **`sTalentStore` (so stock talents can be validated instead of skipped)**, `playercreateinfo`.
5. Write with **prepared statements** (`CharacterDatabasePreparedStatement::SetData`), never string-built SQL; all rows of the
   character (including the marker `coa.portable.import`) in the one transaction.
6. `CharacterDatabase.CommitTransaction(trans)`; **after** a successful commit only:
   `sCharacterCache->AddCharacterCacheEntry(...)`, advance the generators/`_hiPetNumber` exactly as `LoadDump` does,
   `sWorld->UpdateRealmCharCount(account)`.
7. Never touch a character that is online; never overwrite an existing one (replacing is the separate, backed-up operation of
   Phase 4).

## Idempotency and crash recovery

The marker row is written in the same transaction as the character, with the job's nonce. Re-running the same job id finds the
marker, writes the same `.result` and does nothing else. The Manager keeps the same `import_journal` as the offline path, so
the recovery table of `PORTABLE_IMPORT.md` applies unchanged; only the "who writes the realm side" part differs.

## Result file

`<job_id>.result` (JSON, written to a temp name and renamed): `{status, local_guid, item_base, pet_base, renamed, final_name,
counts, not_applied[], warnings[]}` or `{status:"refused", problems[]}` with the same problem codes as the offline preflight. The
Manager turns it into `finish_import`.

## What it must also cover (beyond the offline importer)

Online accounts (the character list on a connected client is stale until relog), the cache refresh above, per-character module
state through the module extension API of Phase 7, collections of the account (Phase 6, keyed by the destination account), and
the level-cap projection hooks of Phase 8.

## Not part of this design

Exports from a running realm (checkpoints need a per-character save command, `.character save <name>`, tracked separately),
a payload over RA, a path argument, a remote caller of any kind.

## Implementation notes (Phase 5)

* **Job format.** The job file is `<header JSON>
<canonical JSON of the character>`, not zstd: the core has neither a JSON nor a zstd
  dependency, and a small strict parser (`CoAPortableJson`: bounded depth/nodes/string size, integers only, duplicate and unknown
  fields refused, UTF-8 checked) is easier to audit than a new dependency. The header carries `snapshot_sha256` of the exact bytes of the
  second line; the file size is capped at 24 MiB before it is read. The Manager ships the character **without extensions** and without
  settings the policy does not carry.
* **Writes.** Every row is written by a prepared `INSERT` (`CHAR_INS_PORTABLE_*`); the only string-built statements are the
  `DELETE ... WHERE <fixed column> = <numeric guid>` that clear leftovers of a deleted character with the same guid.
* **Commit.** `DirectCommitTransaction`, then the marker is read back; the name cache entry, the item/pet/player generators and
  `UpdateRealmCharCount` change only after that read confirms the commit.
* **Talents.** Stock talent rows are written only when the spell is a talent of the realm's own `TalentSpellPos` (the check the worldserver
  asserts on); everything else is reported in `not_applied`.
* **Idempotency.** The marker `coa.portable.import = <nonce words> <revision>` is looked up first; a repeated job id answers `OK` and creates nothing.
* **Session.** With a `session` in the header the same transaction writes the `coa_portable_session` row in state 0, so the character's
  first load takes its baseline (see `PORTABLE_SESSIONS.md`).
