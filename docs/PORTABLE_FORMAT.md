# Portable character format (Phase 1)

What is stored locally and how it is serialised. Code: `crates/coa-core/src/portable/`. Reasoning:
`PORTABLE_CHARACTERS_AUDIT.md`.

**Status:** format version 1 is *provisional* until the Phase 3 gate (Realm A -> Manager -> Realm B) passes. Until then
a change of shape may be made without a migration because nothing outside development machines holds version 1 data.
After that gate every change bumps the version.

## Versions (`versions.rs`)

| Constant | Value | Meaning |
|---|---|---|
| `PORTABLE_CHARACTER_FORMAT_VERSION` | 1 | the serialised `PortableCharacter` |
| `PORTABLE_COLLECTION_FORMAT_VERSION` | 1 | the encoded id set of a collection |
| `EXTENSION_FORMAT_VERSION` | 1 | the extension container |
| `SNAPSHOT_FORMAT_VERSION` | 1 | the stored snapshot envelope (compressed canonical JSON) |
| `REGISTRY_PROTOCOL_VERSION` | 1 | reserved for Phase 10 |
| `RELAY_PROTOCOL_VERSION` | 1 | reserved for Phase 13 |

A reader **refuses** data whose version is newer than it knows (checked before the shape is parsed), and reads older
versions through a migration.

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
`PRAGMA user_version`. A database from a newer Manager, or a foreign SQLite file, is refused and left untouched.

| Table | Purpose |
|---|---|
| `profile` | the owner |
| `character` | summary row + canonical revision |
| `snapshot` | `(character_id, revision)` -> compressed canonical payload + hash |
| `character_server_mapping` | `(character_id, server_id)` -> local guid; `UNIQUE(server_id, local_guid)` |
| `item_mapping` | `(character_id, server_id, portable_item_id)` -> local item guid; `UNIQUE(character_id, server_id, local_item_guid)` |
| `collection` | per profile and kind: revision, hash, encoded set |
| `setting` | `history_keep` |

The store never reads or writes a realm database and never leaves the machine.
