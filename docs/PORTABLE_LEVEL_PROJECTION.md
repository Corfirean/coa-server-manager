# Level-cap projection (Phase 8): design, implementation, verification

Companion of `PORTABLE_LEVEL_PROJECTION_AUDIT.md` (what the core and `mod-coa-content-scaling` actually do; read it first). CoA only.

**Invariant.** The Owner's canonical character is never down-levelled. A realm whose `MaxPlayerLevel` is below the character's level is given a
*working copy* at the cap (`P = apply(hold, C)`); everything the copy cannot hold stays in `C`; a realm that can hold it all gets all of it back.

## 1. Who decides, and about what

Only the running core knows the effective (post-scaling) item levels and holds the CoA progression tables, so **the core decides** and the
Manager applies. The Manager never reads a level requirement.

| Core (`src/server/coa/CoAPortableProjection.{h,cpp}`) | Manager (`crates/coa-core/src/portable/projection/`) |
|---|---|
| `portable project <job_id>`: read-only query over a job file (header `query:"project"`, snapshot) -> manifest in `<job_id>.result` | `ProjectionAnswer::parse` (strict), `Decision::{Native, Projected(ProjectionHold)}` |
| `Project(input, level)`: held items, held abilities, held buttons, held entries of the stored build records, blocked records | `apply(canonical, hold)`; `settings::Record` (structure-only codecs); `merge3_with(.., MergeProjection)` |
| `Signature()`, capabilities report v2 `progression {max_player_level, projection_protocol, projection_policy_version, progression_signature, scaling_enabled}` | profile v2 (`capabilities::Progression`), `ProgressionPin`, `ProjectionContext` |
| import job header `projection {active, level, policy_version, progression_signature}`: verified, **idempotence check** (the core projects the working copy again and it must hold nothing), `coa.portable.pin` written | `realm::project::plan` (the one policy for import, update, reconcile), `CoreOracle`, `SuppliedDecision` |
| login of a session character whose pin is not the core's progression: refused | `HostService` ends the session under the old pin, `reproject_session` |

### What the core holds at level `L` (policy version 1)

* **Items**: a top-level item in an equipment slot (0-18) or a bag slot (19-22) whose *runtime* `RequiredLevel` is above `L` (this is what `_LoadInventory` +
  `CanEquipItem` would otherwise delete from the slot and mail), and the contents of a held bag. Everything else (backpack, bank, bag contents) is written.
* **Abilities**: a spell of the character that appears in a CoA level table (`ClassSpells`, `LegacyGeneratedClassSpells`, `Ranks`, Felsworn rifts, `TaughtAbilities`,
  `TalentReplacements`, talent entries) and **no** source of which allows it at `L` (a source allows it if its level is `<= L`, its talent entry is not held, its parent
  ability is not held). Spells of the live baseline, proficiencies and unresolved trainer spells are level independent and never held. Computed to a fixpoint.
* **Talent entries**: held when `RequiredLevel > L`; paid entries (AE/TE cost) in `(RequiredLevel, EntryId)` order are kept while `Spent` fits `GetCoATalentBudget(class, L)`,
  whole entries only (no partial rank); an automatic entry whose required entry is held is held with it.
* **Buttons**: `character_action` spell buttons showing a held ability.
* **Stored builds** (`core.ascension_slot.<n>`, `core.ascension_build.*`, `core.ascension_bar.*`, also `.slot.<n>.build/.bar`): the held entries/buttons are listed per record;
  a record the core cannot take apart is `blocked`: not given to the realm and never merged back.
* Level `L`, xp `0`. Money, honor, reputation, quests, skills, pets, glyphs, wardrobe, collections, extensions: applied (unchanged by the cap).

If the cap has no talent budget row for a class, the query fails (`projection`): nothing is guessed.

### The progression signature

SHA-256 over: policy version, `MaxPlayerLevel`, the content-scaling switches that decide the layout (`Enable`, `Progression.Mode/ClassicEnd/TbcEnd`, `ScaleItems`), the
compile-time CoA progression tables, the talent data and budgets as loaded, and the sorted `(entry, runtime RequiredLevel)` of every item template (hashed once, lazily, not
the item database). Eras/packs are covered through the item levels they change. It is constant while a worldserver runs; any change means a restart.

## 2. Manager flow

```text
plan(C, revision, options, oracle)
  no capabilities          -> given as is (lower layers, bare tools)
  capabilities, no progression -> refused (unknown is never "no cap")
  level <= cap             -> native; pinned to the realm's progression
  level >  cap             -> oracle.decide(C) (running core via RA, or SuppliedDecision for a stopped realm) -> hold
                              checked: same signature/cap/policy as the realm's profile, made for exactly this snapshot
                              view = apply(C, hold); context = ProjectionContext{canonical_level, projected_level, canonical_revision,
                              content_profile_hash, progression_signature, projection_policy_version, hold}
```

* **Import** (offline and online) and **update** use `plan`; the job/script carries the pin; after the realm committed, `realm_projection` (schema 8) holds the context and
  `character_server_mapping.progression_pin` the pin (cap, policy, signature, content profile hash, projected or native).
* **Update** `merge3(realm now, P_old(synced), P_new(canonical), Strict)`: both ends are views (the stored hold is re-applied to the synced canonical, a fresh decision to the new one);
  build records merge key by key; when the old state was a working copy, the new view's level/xp win over the player's own (`adopt_progression`). A changed pin forces the update
  even at the same canonical revision. A stopped realm needs a supplied decision, else `ProjectionNeedsRunningCore` (nothing is written).
* **Session / reconcile** `merge3(C0, B0, B1, Lenient)` with `freeze_progression` (the realm's level/xp never reach `C`), blocked records ignored, build records merged by key.
  A held item/ability/button is absent from `B0` and `B1`, which the existing three-way merge already reads as "filtered by the realm: kept". Nothing was added to the merge for them.
* **Why key-wise merge of builds.** The core rewrites `core.ascension_slot.<n>` from the live spellbook at every save (audit section 6). Replaced as one value, `B0 -> B1` would erase every
  held entry of `C`. Merged by key (entries, buttons, `(class, spec)` head) the realm's rewrite changes nothing and a pick made at the cap arrives.
* **Session protocol 2.** `PortableSessionStarted.progression {pin, projection?}`, `PortableCheckpoint.pin`, `OwnerAck.pin`. The Owner stores the pin/context at the start (and
  refuses an inconsistent one: another canonical level/revision, a projected pin without its context), then refuses any checkpoint under another pin. `session_protocol` 2 in the
  content profile and the version check on every message make a v1 and a v2 peer refuse each other before a character is armed.
* **The cap moves under a session** (restart with another `MaxPlayerLevel`/rules). The core refuses to let the character in (`coa.portable.pin` is not its progression; read at login
  by query, because `character_settings` sources outside `core.` are not loaded into the player). `HostService::observe_profile` sees the new signature; `tick` reads the realm as it is
  (the old working copy: nobody played it) and sends the final checkpoint **under the old pin**; the Owner merges it as a projection of the old cap. The next session is flagged
  `reproject` and `rearm_pending` does not arm it. `reproject_session` asks for the decision at the new cap, updates the working copy in place (arming the next session in the same
  transaction) and clears the flag. No message ever merges across two progressions.

## 3. Classification of the other state

Apply: money, honor, arena points, currencies, titles, explored zones, reputation, rewarded and active quests, skills, glyph rows (none exist), pets (the core levels a pet to its owner at
load: normalisation, not progress), wardrobe, collections, extensions, macros. HoldByProjection: items, abilities, buttons, build entries as above. Projection-owned: level, xp.
Unsupported: stock talent rows (never written, as before), a build record that does not parse (blocked, kept canonical). Evidence per row: audit sections 4-7.

## 4. Capability profile v2

`profile_version` 2 adds `progression` (outside the content hash) and `Feature::LevelProjection`; core report v2 carries `progression` (a v1 core reports none). A stored v1
profile is **migrated** (`from_json`): content and hash are unchanged, `progression = None`, which compat reads as Blocking "probe the realm through its core" for every operation. `ProfileChange::progression_changed`
reports a moved cap/signature even when the content hash did not move.

## 5. Verification

Unit (Manager, `cargo test -p coa-core --lib`): codecs, `apply`, activation matrix (80->80 none, 80->70, 80->60, 70->60, 60->60 none), strict answer parsing, key-wise merge incl. idle session
equals canonical, repeated and later checkpoints, blocked records, level freeze, adoption, pins/contexts in the store, fake-realm sessions (whole projected session, forged pin, wrong projection,
cap change under a session, online at the change), profile v1 migration, compat outcomes.

Live (`#[ignore]`, real MySQL A/B): `realm::live_projection` (offline import at cap 60 with a supplied decision, offline play + reconcile with the realm's level moved and a new item,
update to a cap-80 profile restoring helmet/abilities with the canonical byte-identical, down to 70 and 60 again).
Live with a **real worldserver** (`session::live_projection`, cap-60 build of the fork, optional cap-70 configuration, fixture ranger or a real level-80 character): the core refuses an
unprojected level-80 job (`above_level_cap`), an inconsistent working copy (`projection_inconsistent`) and another progression (`progression_changed`); the real load mails nothing the
projection held; the Host takes `B0` under the pin; the realm-side level is moved (59) and the canonical level/xp stay; checkpoints and the final checkpoint keep held items, abilities and
build entries; after a restart at cap 70 the core refuses the character, the Host ends the session under the old pin, `reproject_session` brings the working copy to 70 and restores exactly
what 70 allows.

## 6. Limits

* A stopped realm cannot be projected without a decision supplied from its core (`portable_smoke project ... --out`, then `--projection`).
* Whole-entry hold of talents (a partly affordable entry is held entirely); the budget order is deterministic, not optimal.
* Offline (non-runtime-session) play is not guarded against a cap change in between; run `update` after a cap change (the pin makes it write) before playing. Runtime sessions are guarded by the core.
* Item holding is by level only (equipment and bag slots), as in the audit; skill/proficiency requirements stay the realm's own business.
* No conflict-resolution UI, Registry, Relay, Wildcard, personal bank or equipment sets in this phase.
