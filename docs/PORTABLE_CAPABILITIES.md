# Capability negotiation and extensions (Phase 7)

Code: `portable/capabilities.rs`, `compat.rs`, `extension.rs`, `realm/profile.rs`, `store/profile.rs`, `realm/reconcile.rs`
(`reevaluate_realm_character`). Not in this phase: Registry, Relay, any network transport (the profile is plain data a Registry can
publish later), Wildcard, the personal bank, equipment sets, level-cap projection, and an adapter for any real module.

## 1. The content profile

`RealmCapabilities` is what a destination can take, transport-neutral and small (a few hundred bytes of hashes, no id lists):

```text
RealmCapabilities
  profile_version                  1 (a newer one is refused before the shape is read)
  core { commit, branch, date }    identity of the realm's core build; for logs and support, not part of the hash; absent when the core was not asked
  content: ContentProfile          everything that decides what can be applied
    ruleset                        coa (Wildcard transfer is disabled)
    character_formats              readable 1..2, writable 2 (the Manager side that serves the realm)
    online_import_job_formats      what the realm's core reads for an online import (empty: not asked / too old to answer)
    session_protocol               1   (PortableSessionStarted / PortableCheckpoint / OwnerAck)
    collection_protocol            1   (CollectionObserved / CollectionState / CollectionAck)
    features                       runtime_sessions, wardrobe, collections
    collection_kinds               coa:appearance, coa:vanity
    extensions                     [{namespace, module_version, formats: {min, max}}]  sorted by namespace
    client_catalog                 {Appearances.dbc, ItemAppearances.dbc, VanityCollection.dbc, ItemSet.dbc} -> {sha256, records}
  content_profile_hash             SHA-256 of the canonical JSON of `content`
```

* **Hash, not lists.** Two realms with the same `client_catalog` hashes know exactly the same appearances and vanity items. The ids
  themselves are read, where they are needed (the side that writes to the realm), from the realm's own data directory
  (`RealmKnowledge::from_data_dir`), and that read carries the same hashes: an evaluation blocks when the files the Manager read are not
  the ones the profile (and the core) describe.
* **The hash covers content only.** A new core build with the same content has the same `content_profile_hash`; a different client
  table, a feature switched off, another extension range, another job format or protocol version moves it. A profile from outside is
  size-checked, version-checked, strict (`deny_unknown_fields`) and its hash must be the content's: a forged or edited one is refused.
* **Sources.** The core (RA `portable capabilities`: commit, branch, date, the job formats it reads, the session marker version, its
  features, and the SHA-256 and record count of the client tables **it loaded**), the realm's schema (which module tables exist), the
  data directory (the catalog), and the registered extension adapters. `probe_capabilities()` assembles them; when the core cannot be asked
  (a stopped realm) the profile says so by the absence of `core`, empty job formats and no `runtime_sessions`: the Manager never guesses a
  capability it did not see. When the core was asked, its catalog must equal the one hashed from the data directory, table by table.
* **Remembered.** `realm_profile` (schema 7) keeps the last profile of every realm and whether it came `live` or `offline`.

## 2. Preflight: explicit outcomes before anything is written

`compat::evaluate` returns, for a character and an operation (`OfflineImport`, `OnlineImport`, `Update`, `RuntimeSession`), one `Outcome` per
topic and an overall `Verdict`:

| Outcome | Meaning |
|---|---|
| `Compatible` | applied completely |
| `Held` | partly or wholly **not applied** (ids the realm's client data does not know, an extension in another format); stays in the canonical character, with counts and the reason |
| `Unsupported` | the realm lacks the feature; nothing applied, nothing lost |
| `Blocking` | the operation cannot be done on this realm: another ruleset, a character format the realm cannot read, an online import the core cannot take (another job format, or the core was not asked), a session it cannot run, client data that is not what the realm loaded |

`Verdict`: `Incompatible` (any blocking), `Degraded` (anything held or unsupported), `Compatible`. The operations that write
(`import_character*`, `import_character_online[_on]`, `update_realm_character*`) call it **first** when the options carry the profile
(`ImportOptions::capabilities`) and refuse with `PortableError::Incompatible { operation, reasons }` before the first row, journal entry or job file;
what is held back is returned in `not_applied` / `warnings`. Without a profile nothing is evaluated (the lower layers' own tests, tools
that do not know the realm); the harness always assembles one (`--data-dir`, plus `--ra-port` for the core).

## 3. Extensions

In the canonical character an extension (`mod:<module>`) is an **opaque, hash-protected blob**: the Owner never interprets, edits, drops or applies it.
`ExtensionAdapter` is the only thing allowed to touch a module's data on a realm:

```text
namespace()                      mod:<module>
module_version()
supported_format_versions()      {min, max}
available(realm)                 is the module on this realm
validate(extension)              well formed? (also run on everything an adapter exports)
export(realm)                    the module's data of this character, or none
apply(realm, extension)          write a validated, compatible payload
compatibility(extension, profile)  default: the format check against what the profile published; an adapter may be stricter
```

`ExtensionRegistry` registers adapters (only `mod:` namespaces, one per namespace, a sane range), lists what a realm supports (`supported_on`),
evaluates (`evaluate`), exports (`export_all`, validated) and applies (`apply_all`). Rules, each covered by a test with the fake extension:

* **A realm without the extension** (no module, or no adapter): the profile does not list the namespace, the outcome is
  `Deferred(MissingOnRealm)`, the destination receives **nothing**, the canonical payload is untouched, and a later realm that has the module
  receives it.
* **An incompatible format** (older or newer than the realm's range): `Deferred(FormatTooOld | FormatTooNew)`; nothing is written; the payload stays.
* **A failing or lying adapter** (`apply` fails, a payload fails its own validation or its content hash): `Failed`, nothing is claimed, the next attempt starts again.
* **Applied once.** What was applied is remembered by content hash (`realm_extension_state`); an unchanged payload is not written again, a changed one is.
* **The Manager's own blobs** (`coa:*`, e.g. quarantined settings) are canonical-only and never handed to an adapter.
* **Merge.** A realm contributes only what its own adapter exported (its `B0` and `B1` hold nothing else) and only what changed between them;
  a namespace nobody on that realm understands is not in `B0`/`B1`, so it is not in the delta and survives every session unchanged.
  The Host reads adapter exports into baselines and checkpoints (`LiveBridge::with_extensions`).

No adapter for a real module exists; the framework is exercised by a fake module only.

## 4. The profile follows the mapping

`character_server_mapping.content_profile_hash` (schema 7) is the hash the realm had when the character was last synchronised with it (set
after every import, update and re-evaluation). When the realm's profile changes:

* `HostService::observe_profile` records the new profile; if the hash moved, what the Host sent and received of the **account collections** is forgotten
  so the next look reports every account again and the Owner answers with its canonical sets (the realm may now know ids it did not), and the characters
  whose synchronised hash is not the new one are returned (`mappings_with_other_profile`);
* `reevaluate_realm_character` looks again at what was held back for such a character **at the same canonical revision**: appearances (selections, outfits)
  the realm can now show and does not have are written, extension payloads that now have an adapter are applied; **only additions**, nothing the realm
  has or changed since is touched; the new hash is recorded. With the same hash it does nothing; with a new hash and nothing new to show it writes nothing.

## 5. Verification (Phase 6.1 and 7 gate)

| What | Result |
|---|---|
| `cargo test --workspace --locked` | see section 6 |
| Format 1 -> 2 | genuine v1 migrates with an empty wardrobe and its original hash is verified first; v1 with a wardrobe is rejected; wrong hash / unknown field / version 0 / non-numeric refused; stores holding v1 snapshots read them and compare by `semantic_hash` (no spurious revision, the Host accepts the same revision in the new format) |
| Job format | the core refuses `job_format` 99, absent, non-numeric and 1, with `supported_job_formats`, before parsing the body, nothing written (real worldserver) |
| Profile | stable, strict, forged hash refused; assembled from a real schema + data directory; the core's report cross-checked with the data directory (a different `Appearances.dbc` is refused); a profile is under 4 KB |
| Preflight | another ruleset, a reader of format 1 only, a core that reads another job format, no session feature: refused with the reason and **not one row, mapping or journal entry written** (real MySQL, real worldserver); a partly compatible one proceeds and reports what is held |
| Profile change | simulated: collections reported again and the 200 held-back ids written without a new revision; real MySQL: held-back selection restored at the same revision, the realm's own choice untouched, a second call and a call with nothing new write nothing |
| Extensions | fake module: absent on realm / incompatible old and new / failing adapter / rubbish payload / unknown namespace / `coa:*`; opaque through sessions and restored on a later realm that has the module |

## 6. Limits

* The adapter exports are read by the Host's baselines and checkpoints (`LiveBridge`, the simulated realm); the Phase 4 offline `begin_session` / `reconcile_session`
  functions do not take adapters.
* The online import applies extensions only when it is given the realm's database connection (`import_character_online_on`); `import_character_online` without it
  reports them as not applied.
* A held-back **selection** is restored by a re-evaluation (a profile change); a held-back **extension** is applied by it; nothing is re-applied when the profile did not change.
* Whether the profile of a running realm still matches (a core upgraded in place) is noticed only when it is read again; there is no push.
* The Host driver is still a library plus the harness; the profile is not yet fetched by the Manager application on its own.
