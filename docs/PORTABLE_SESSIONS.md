# Runtime portable sessions (Phase 5): design and audit

Code: core `src/server/coa/CoAPortableSession*.{h,cpp}` and `CoAPortableImport*.{h,cpp}` (branch `feat/portable-session-bridge` of the
core fork), Manager `crates/coa-core/src/portable/session/`. CoA only; Wildcard portable transfer stays disabled.

Phase 4 proved the offline model with a real client. Its one fragile rule is that the baseline `B0` must be captured *after*
the realm's own normalisation and *before* the character is played. Phase 5 makes that rule a property of the running
server instead of a user procedure:

```text
running worldserver -> automatic B0 -> 60-second checkpoints -> Owner canonical revisions -> final logout checkpoint
```

## 1. Roles (the API reflects the real deployment)

| | OWNER (player's Manager) | HOST (the realm's Manager) |
|---|---|---|
| owns | canonical `PortableCharacter`, revision history, `C0` per session, applied-checkpoint log | realm binding, item/pet mappings, synced snapshot of what the realm was given, outbox |
| does | validates every message, runs `merge3(C0, B0, B1)`, writes revision N+1 | controls the local worldserver, captures `B0`/`B1` from the realm, emits messages |
| never | touches a realm | opens or mutates the Owner's database; holds canonical history |

Both roles run on `Store` (one schema) in this phase, but the APIs are `OwnerService` and `HostService`, they exchange only
the messages of section 5, and every test runs them on two separate stores (two files, two processes' worth of state).
A later Registry/Relay carries the same messages unchanged.

## 2. Audit of the login path (what the core actually does)

`WorldSession::HandlePlayerLoginFromDB` (`Handlers/CharacterHandler.cpp`) is one synchronous function on the world thread:

1. `Player::LoadFromDB` (the realm's normalisation: unknown spells deleted, default spells/skills/reputation added, honor and title
   resets, items that cannot be placed mailed or dropped);
2. `SendInitialPacketsBeforeAddToMap`, `AddPlayerToMap`, `SendInitialPacketsAfterAddToMap`, social/guild/group announcements,
   `LoadCorpse`, at-login flags (`resetSpells`, `resetTalents`, first-login spells), `LoadPet`;
3. **`sScriptMgr->OnPlayerLogin`** and **`OnPlayerFirstLogin`** (module normalisation, e.g. the starter-kit restore) as the
   *last* statements.

`OnPlayerLogin` is therefore **not** early (it is the end of the function) but the end of the function **is** the right point:
everything the realm does to a loading character has run, and nothing the player does can have run, because

* client packets are handled in `WorldSession::Update`, which cannot run before this handler returns (same thread), and
* the map's `Update` (mobs, auras, regeneration, `Player::Update`) runs later in the same world tick.

So the character state at the end of the handler is exactly "normalised, not yet played". Bots enter through the same function
(`BotMgr` calls `HandlePlayerLoginFromDB`), so the same code serves tests with bot-logged characters. The "login after
disconnect" shortcut (`HandlePlayerLoginToCharInWorld`, a player that never left the world) does not reload the character and
does not start a session.

Other facts that shape the design:

* `Player::SaveToDB(trans, create, logout)` is the single place that writes a character; manual save, autosave, logout save,
  `.saveall` and checkpoints all go through it. It returns early (a delayed save) while `IsBeingTeleportedFar()`, and it saves the
  pet through a **separate** transaction (`Pet::SavePetToDB`). With `CharacterDatabase.WorkerThreads = 1` (the default, and
  enforced by the portable subsystem at startup) transactions commit in FIFO order, so the pet's commit precedes ours.
* `CommitTransaction` is asynchronous: an RA reply is not persistence. The Host Manager reads the database and treats a save
  as having happened only when it **sees the marker row**.
* The save is a snapshot taken when `SaveToDB` runs, not when the commit lands. That is what makes `B0` exact.
* RA/console commands run on the world thread (`QueueCliCommand`), so a command can safely touch a `Player*`.
* The character list is stale on already connected clients after an import; `CharacterCache` and
  `World::UpdateRealmCharCount` must be updated (the importer does, section 7).
* `PlayerDumpReader::LoadDump` is the precedent for the importer's runtime invariants (live generators advanced only after the
  commit, name check, cache entry, realm character count).

## 3. Session marker (core table `coa_portable_session`)

One row per portable character on the realm (pending SQL `data/sql/updates/pending_db_characters/`):

| column | meaning |
|---|---|
| `guid` | local character guid (primary key) |
| `session_id`, `character_id` | UUIDv7 strings: the session and the portable character |
| `imported_revision` | canonical revision `C0` that the realm holds (`base_canonical_revision`) |
| `baseline_generation` | increments each time the session is (re)armed |
| `state` | 0 `waiting_baseline`, 1 `baseline_ready` (gated), 2 `active`, 3 `ended` |
| `checkpoint_seq` | the last checkpoint sequence the core saved |
| `save_seq` | incremented by **every** save of this character |
| `updated_at` | unix time |

The Host arms a session (state 0) in the same realm transaction that imports/updates the character (offline importer, in-place
update) or the core's import service does (online). That is the "pending portable-session marker" the character carries until
its first load.

### Automatic B0

At the end of `HandlePlayerLoginFromDB`, if the character's row is in state 0, the core, in **one** transaction:
`SaveToDB(trans, false, false)` + `UPDATE coa_portable_session SET state=1, baseline_generation=..., save_seq=save_seq+1`,
commits, and **gates** the session:

* while gated the session's packet queue is not processed (the client stays connected; `CMSG_PING` is answered by the socket
  layer), the character is made non-attackable and rooted, nothing the client sends is lost - it is processed after release;
* the gate ends when the Host sends `portable release <session_id>` (the Host has seen `state=1`, exported `B0` in one read-only
  consistent snapshot, verified inside that snapshot that the marker is still the baseline's, and sent `PortableSessionStarted`);
* if no release arrives within `PortableSession.GateTimeoutSeconds` (default 30) the character is **kicked**, never released:
  no gameplay may happen before `B0` exists. The row stays in state 1; the next login re-runs the baseline.

Characters whose row is in state 2 (a worldserver restart or a relog in the middle of a session) get no baseline, only the
in-memory registration; `B0` and `C0` are already held by the Owner.

### Every save carries a marker

`Player::SaveToDB(trans, ...)` appends, for a registered portable character, `UPDATE coa_portable_session SET
save_seq=save_seq+1, updated_at=...` to the **same** transaction (plus `checkpoint_seq=N` when the save is a checkpoint and
`state=3` when it is the logout save). So the rows of a character in a read snapshot always belong to exactly the save whose
marker is in that snapshot; an autosave between two checkpoints is visible as a higher `save_seq`.

### Checkpoint (typed core operation)

`portable checkpoint <local_guid> <session_id> <sequence>` (console/RA only): the character is addressed by guid, never by name;
the command verifies the player is online, a portable session is registered, `session_id` matches, the state is 2, the
sequence is greater than `checkpoint_seq`, and the character is not mid-teleport (`BUSY`, retry). Then **one** `SaveToDB`
transaction with the checkpoint marker, committed. The reply is `QUEUED <sequence>`; it is **not** an acknowledgement of
persistence. `.saveall` is never used, so one checkpoint writes one character (verified, section 9).

Logout: the logout save itself carries `state=3`; the Host exports and sends the final checkpoint.

## 4. Host flow

```text
import / update of a portable character  ->  arm session (state 0), outbox: nothing yet
poll realm every ~1 s (cheap indexed read of coa_portable_session):
  state 1, no PortableSessionStarted sent  ->  read B0 (consistent snapshot, marker re-checked) -> persist -> release (RA)
                                              -> emit PortableSessionStarted
  state 2, online, every 60 s              ->  RA checkpoint(seq) -> wait for checkpoint_seq == seq -> read B1 ->
                                              persist in outbox -> emit PortableCheckpoint(seq)
  state 3 (logout) or character offline    ->  read final B1 -> emit PortableCheckpoint(final) -> on ack: re-arm (state 0)
```

A failed intermediate checkpoint changes nothing canonical and is retried at the next tick. The sequence number is persisted
**before** the RA call, so a Host restart resumes with the same number (the marker tells whether the save happened).
Messages wait in the outbox until acknowledged; redelivery is harmless (section 6).

## 5. Messages (transport-neutral, versioned, JSON)

`protocol_version = 1`; each message is canonical JSON with unknown fields refused. Snapshots travel in the existing
snapshot envelope (zstd of canonical JSON, SHA-256 of the canonical JSON).

```text
PortableSessionStarted { protocol_version, session_id, character_id, server_id, base_canonical_revision,
                         baseline_generation, b0: envelope, content_hash }
PortableCheckpoint     { protocol_version, session_id, character_id, server_id, base_canonical_revision,
                         sequence, final_checkpoint, realm_snapshot: envelope, content_hash }
OwnerAck               { protocol_version, session_id, sequence, outcome, canonical_revision, canonical_hash,
                         [final: canonical snapshot envelope] }
```

`outcome` is `applied`, `duplicate` (same sequence and hash, already applied), `stale_sequence`, `stale_session`
(superseded: another session or revision), or `rejected(reason)`.

The Owner validates, **before** `merge3`: protocol version; message size and decompression limits; `content_hash` against the
payload; `character_id` exists and the ruleset is CoA; the session is known (opened by `PortableSessionStarted` whose
`base_canonical_revision` equals the revision the Owner recorded as `C0` for that `server_id`); `server_id` and
`character_id` match the session; the payload's own `character_id`/ruleset match; the sequence.

## 6. Ordering, idempotency, no double counting

* Per session the Owner keeps `last_sequence` and a log of `(sequence, content_hash, resulting_revision)`.
* sequence = `last_sequence + 1`: applied. Same sequence with the same hash: `duplicate`, nothing happens, the original result
  is returned (lost acknowledgements are healed by redelivery). Same sequence, different hash: `rejected`.
  `sequence <= last_sequence` otherwise: `stale_sequence`, ignored. A gap (`sequence > last_sequence + 1`) is accepted only when
  the Owner holds a later state (checkpoints are snapshots, not deltas, so skipping one loses nothing).
* **Every checkpoint is `merge3(C0, B0, current B1)`**, anchored to the session's original `C0` and `B0` (Phase 4's
  property), never `merge(previous canonical, previous B1, B1)`. A repeated or re-sent state therefore cannot add money, honor or xp
  twice; the canonical revision can advance while the anchors stay.
* A superseded session (the character's canonical revision no longer equals the session head) refuses checkpoints
  (`stale_session`); no divergent-state merging is invented here.
* Crash: the last acknowledged checkpoint is canonical. A worldserver crash leaves the row in state 2 and the character offline:
  the Host marks nothing, the next login continues the same session (no new baseline). A final export after a crash is a
  normal offline export of the last committed save.

## 7. PortableImportService (production import without stopping the realm)

Follows `PORTABLE_IMPORT_SERVICE.md` with one concrete decision: the **job file is canonical JSON, not zstd**, because the core
has no zstd or JSON dependency and a hand-rolled bounded strict parser is easier to audit than adding either. The size cap
(16 MiB, as the Manager's decompression cap) is enforced before parsing; the header carries the SHA-256 the file must match.

```text
Manager writes <JobDir>/<job_id>.job (temp name + rename)
-> RA: portable import <job_id>        (job_id = UUIDv7, nothing else)
-> world thread: validate -> allocate from live generators -> prepared statements -> ONE CharacterDatabaseTransaction
   (character, items, spells, ..., session marker in state 0) -> commit
-> after the commit only: CharacterCache entry, generators advanced, UpdateRealmCharCount
-> <JobDir>/<job_id>.result (temp + rename); RA reply "OK <job_id>" or "ERR <code>"
```

No path, SQL or snapshot travels through RA. Idempotency: the marker row carries the job nonce; re-running a finished job
rewrites the same result. Never touches a character that is online or already exists.

## 7a. Behaviour of the implementation worth knowing

* The gate holds the **session's packet queue** (`WorldSession::Update` skips it while gated) and makes the character non-attackable
  and rooted. The timeout is enforced from a world-update hook, not from the session, so sessions without a socket are covered too; a
  real client is kicked, a bot session (no socket) is only reported.
* A **clean shutdown logs every player out**: the logout save of each portable character carries `state = 3`, exactly like a normal
  logout, and the Host sends the final checkpoint and arms the next session. A **crash** leaves `state = 2`: the next login continues the
  same session with no new baseline.
* A character that logs in while its row is still `state = 3` (the Host has not closed the session yet) is disconnected with a
  "try again in a moment" message: a baseline would otherwise be taken against a session the Owner already closed.
* The session table is read at login by the login query holder (`CHAR_SEL_PORTABLE_SESSION`), so a realm whose database lacks
  `coa_portable_session` refuses to start like any other missing table: the pending SQL ships with the core.
* One checkpoint writes one character: verified on a real worldserver with two other characters online by counting the `characters`
  rows the database was asked to write.

## 8. What is not in this phase

VPS Registry/Relay and any network transport, public server list, remote account provisioning, Wildcard, vanity/wardrobe
transfer, personal Ascension bank, equipment sets, level-cap projection.

## 9. Verification plan

* Owner/Host on two stores: canonical revision 10 -> running realm -> automatic `B0` before gameplay -> play -> checkpoint 1
  (rev 11) -> same checkpoint again (still 11) -> more play -> checkpoint 2 (rev 12) -> delayed checkpoint 1 refused ->
  final checkpoint.
* Money/item/spell/quest/pet progression; realm normalisation not propagated; retry; lost acknowledgement; Host, Owner and
  worldserver restart; stale session; wrong character or session; malformed message; over-limit payload; logout during a
  checkpoint; disconnect/crash before the final checkpoint.
* Real worldserver with the character online and several checkpoints without logout; the number of `characters` rows written
  during a checkpoint is counted on the database (general log) to show one character is saved, not all.

## 10. What was verified

Manager: 413 workspace tests (the session layer alone has 15 with a simulated realm that normalises characters, holds the player until
the baseline is released, saves on request and crashes on demand), `check-i18n`, `npm run build`. Owner and Host are two separate stores in every
test; restarts of either are tests of their own (files closed and reopened between steps).

Live, two disposable MySQL servers and **a real worldserver built from the core branch** (`feat/portable-session-bridge`, commit `aeba87f04`,
not pushed):

* `a_running_worldserver_takes_its_own_baseline_and_checkpoints_one_character_without_a_logout`: the character is imported with its session
  armed, logs in (`botcmd spawnbot`, the real `HandlePlayerLoginFromDB`) next to two other online characters, is held at `state = 1`, the Host
  takes `B0` and releases it, then three **real** level changes each produce one checkpoint **without a logout**; the database's general log shows
  exactly one `characters` row written per checkpoint (the character's own), the Owner gets revisions 2, 3, 4 with the real level, and what the realm
  deleted at load is still in the canonical character. Then logout -> final checkpoint -> the next session armed -> the next login takes its baseline
  by itself again; a worldserver **crash** in the middle of a session is continued without a new baseline and without gate; a **clean shutdown**
  ends the session like a logout and the Host sends the final checkpoint.
* `a_character_whose_host_never_answers_is_never_released_and_the_core_disconnects_it`: no Host, no release: the core gives up after the
  gate timeout, the row stays in `state = 1`, nothing was played.
* `the_core_imports_characters_into_a_running_realm_like_the_offline_importer_does`: four characters imported into the **running** realm; each
  reads back identical to what the offline importer makes of it; the id generators, the name cache (a command by name finds the new character), the
  account's realm character count and an immediate login all work; the same job id twice creates nothing; an unknown item, a missing account, a
  tampered snapshot and garbage are refused and write nothing.
* the earlier live suites (export, offline import, reconcile, in-place update, round trip) still pass with the new table in the schema; the update
  also arms the next session in the same realm transaction (`an_in_place_update_arms_the_next_runtime_session_in_the_same_transaction`).

Core: `python -B tools/verify_all.py --stages source --base origin/coa-bots` passes (comments, boundaries, registrations); the C++ code style
check passes on the changed files. The build, unit, harness and gameplay stages were not run: a worldserver was built and exercised through the live
tests above instead.

Known limits: a bot session has no socket, so the gate's packet hold and kick are only exercised for real clients by a manual test; the
core's job parser is exercised through the live importer test, not through its own unit tests.
