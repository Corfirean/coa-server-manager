# Portable character format (Phase 1)

What is stored locally and how it is serialised. Code: `crates/coa-core/src/portable/`. Reasoning:
`PORTABLE_CHARACTERS_AUDIT.md`.

**Status:** the Phase 3 gate passed with format 1; since then **every change of shape bumps the version**. Phase 6 added the optional
`wardrobe` section, so the character format is now **2** (Phase 6.1); version 1 is still read, through the strict migration below.

## Versions (`versions.rs`)

| Constant | Value | Meaning |
|---|---|---|
| `PORTABLE_CHARACTER_FORMAT_VERSION` | 2 | the serialised `PortableCharacter` this build writes |
| `PORTABLE_CHARACTER_MIN_READ_VERSION` | 1 | the oldest character format this build still reads |
| `ONLINE_IMPORT_JOB_FORMAT_VERSION` | 2 | the online import job (`<job_id>.job`) the core reads; checked by the core before it parses the body |
| `PORTABLE_COLLECTION_FORMAT_VERSION` | 1 | the encoded id set of a collection |
| `EXTENSION_FORMAT_VERSION` | 1 | the extension container |
| `SNAPSHOT_FORMAT_VERSION` | 1 | the stored snapshot envelope (compressed canonical JSON) |
| `REGISTRY_PROTOCOL_VERSION` | 1 | reserved for Phase 10 |
| `RELAY_PROTOCOL_VERSION` | 1 | reserved for Phase 13 |

A reader **refuses** data whose version is newer than it knows (checked before the shape is parsed), and reads older
versions through a migration.

### Character format 1 -> 2 (Phase 6.1)

Format 2 is format 1 plus the optional `wardrobe` section (absent when empty, so an empty one serialises to the same bytes as before apart from
`format_version`). The migration is strict and lives in `snapshot.rs`:

1. the payload is decompressed under the size caps and its SHA-256 is verified against the **stored** hash **before** anything is
   interpreted or migrated (the original format 1 hash is what was recorded; a payload that does not match it is corrupt);
2. `format_version` is read first; newer than 2 is refused, 0 or non-numeric is refused;
3. a format 1 payload that contains a `wardrobe` (even an empty object) is **rejected**: format 1 had none, so it is not a format 1 payload;
4. a genuine format 1 payload becomes a format 2 character with an empty wardrobe; `deny_unknown_fields` still applies to everything else.

Stored snapshots are never rewritten. The same character has a different hash in the two formats, so anything that asks "is this the same
character?" compares `snapshot::semantic_hash` (the hash of the character as format 2 writes it, for a stored payload of either version) and
never a stored hash with a freshly encoded one (the owner's checkpoint head check, the reconcile head check, the Host's same-revision check).

### Online import job format 2

The header gained `job_format` (first field). The core reads the header line first and refuses a job whose `job_format` it does not read, or that
has none (what Phase 5 wrote), with `unsupported_job_format` and the list of formats it reads, **before** it computes the body hash or parses
the body; the body must be character format 2 (`wardrobe` optional).

## Identity

* `CharacterId`, `ProfileId`, `PortableItemId`, `PortablePetId` are **UUIDv7**, generated locally. Only version-7 ids
  are accepted from text or JSON; a v4 or nil UUID is rejected as foreign.
* Local AzerothCore guids never appear in the model. They exist only in the mapping tables
  (`character_server_mapping`, `item_mapping`), per realm.
* Game content is addressed as `<namespace>:<kind>:<id>` (`ContentId`), e.g. `core:wotlk:item:19019`, `coa:class:12`,
  `mod:<module>:item:7`. The last two segments are kind and number; everything before is the namespace. Plain numbers
  elsewhere in the model (spells, quests, factions, skills) are interpreted in the character's `content_namespace`.

## Canonical form and hash

1. The model is **normalised**: every set is sorted and de-duplicated (spells, talents, skills, glyph sets, items by
   container/slot, quests, reputation, action buttons, pets and their spells, enchantments).
2. It is serialised as compact JSON with a fixed field order (`serde`), unknown fields refused on read.
3. `content_hash = SHA-256(canonical JSON)`. Equal characters have equal hashes; the hash does not depend on the
   compressor.
4. The stored payload is `zstd(canonical JSON)`.

## Limits (hostile input)

`model::limits` caps every list and string; the snapshot is capped at 16 MiB decompressed and 8 MiB compressed, and
decompression stops at the cap while reading (decompression bombs). Money is capped at the realm's own maximum
(2,147,483,646 copper). Items must form a valid container tree (containers directly on the character, no two items in
one place, no duplicate ids). Extension payloads must match their recorded hash.

## Revisions

* `character.revision` is the canonical revision; freshness is decided by this integer, never by a timestamp.
* `commit_snapshot(expected_revision, ...)` succeeds only if `expected_revision` equals the canonical revision
  (`StaleRevision` otherwise) and creates revision + 1, in one `BEGIN IMMEDIATE` transaction.
* `rollback_to` creates a **new** revision whose state equals an old one; revision numbers never decrease.
* The newest `history_keep` revisions are kept (default 20, 1..=1000, configurable). The current revision is never
  pruned.
* A character's `ruleset` (`coa` | `wildcard`) can never change.

## Extensions

`extensions` maps an extension namespace (`mod:<module>`) to `{module_version, format_version, content_hash, payload}`.
The payload is opaque bytes: it is stored and returned unchanged, whether or not this Manager understands it. A realm
that does not support an extension simply does not receive it; the canonical copy is kept.

## Collections

Account-wide sets of numeric ids (`coa:wardrobe`, `coa:vanity`, ...), stored per profile and kind:

* union only: `stored = stored U incoming`; nothing is ever removed because a realm did not return it;
* if the union equals what is stored, **no revision is created** and the hash is unchanged;
* encoding: `[format version][count][first id][gap]...`, unsigned LEB128 varints, strictly ascending ids, 1-2 bytes per
  id for realistic sets (50,000 scattered ids < 100 KB);
* `collection_hash = SHA-256("coa-collection-v1\0" + kind + "\0" + encoding)`.

## Store (`%LOCALAPPDATA%\CoAServerManager\portable\portable.db`)

SQLite, WAL, `synchronous=FULL`, `foreign_keys=ON`, `application_id = 0x434F4150`, forward-only migrations tracked in
`PRAGMA user_version` (currently 3). A database from a newer Manager, or a foreign SQLite file, is refused and left untouched.

| Table | Purpose |
|---|---|
| `profile` | the owner |
| `character` | summary row + canonical revision |
| `snapshot` | `(character_id, revision)` -> compressed canonical payload + hash |
| `character_server_mapping` | `(character_id, server_id)` -> local guid; `UNIQUE(server_id, local_guid)` |
| `item_mapping` | one row per mapping with a lifecycle (schema 2, see below) |
| `import_journal` | schema 3: one row per import into a realm (`prepared` / `committed` / `aborted` / `needs_attention`) with the nonce/marker and the plan; see `PORTABLE_IMPORT.md` |
| `collection` | per profile and kind: revision, hash, encoded set |
| `setting` | `history_keep` |

## Item mapping lifecycle (schema 2)

A realm's item guid counter restarts at `MAX(guid)+1`, so a freed guid can be given to a different item. A bare
`guid -> portable item` mapping would attach the new item to the old portable id. Every mapping therefore records what
the item *was* (`entry`, `identity`) and has a state:

* `active`: at most one per portable item and at most one per local item guid
  (`UNIQUE(character_id, server_id, local_item_guid)` and `UNIQUE(.., portable_item_id)`, both for active rows);
* `retired` (kept for the record) with a reason: `guid_reused`, `moved`, `absent`, `character_rebound`.

`Store::reconcile_item_mappings` takes the complete observation of a character on a realm and confirms, adds or retires
mappings; `Store::resolve_item_ids` answers `Known`, `Reused` (recycled guid) or `Unmapped` for a local guid and its
current identity. Schema-1 rows are migrated with an empty identity, which matches nothing: they are re-verified, never
trusted. Rebinding a portable character to another local guid retires its item mappings.

The identity covers what cannot change during an item's life (entry, random property, crafter); two different items that
share all of these are interchangeable for mapping purposes.

The store never reads or writes a realm database and never leaves the machine. (Phase 2's realm reader lives in
`portable/realm/` and is documented in `PORTABLE_EXPORT.md`; it only reads.)
