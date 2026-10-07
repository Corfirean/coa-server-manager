# Portable characters in the Manager (Phase 9): Player Mode flow, service, runtime, diagnostics

Integrates the proven engines (store, import/update/reconcile, sessions, level-cap projection) into the application without changing them. Local machine / LAN only: no
Registry, Relay or public browsing, no Wildcard, no personal bank or equipment sets. Rust is authoritative; the screen observes state and asks for high-level operations.

## 1. What the player does

Welcome -> *Connect to another server* -> **Characters**. Nothing here needs a local server.

1. **Realms** lists the realms the Manager knows: the servers it installed itself (*installed*) and realms added from a descriptor file (*prepared*, section 4).
2. **Make an existing character portable** reads a character that is *not in the game* from a realm's database and registers it. The Manager keeps a trusted copy
   (the *canonical* character) and remembers which realm it came from. The realm's own character is not touched.
3. **Play on realm...** shows the compatibility check first (section 3), then **Play**. One click prepares everything: the character is put on the realm if it is not there
   (an update if it is behind), the session is armed, and the screen says *Ready. Start the game and log in with this character*. **Start the game** launches the linked client.
4. While playing, the Manager checkpoints in the background; on logout it saves the final state. The card shows the resulting saved version and when it was saved.
5. **Saved versions** lists the canonical history in words ("Progress saved while playing on <realm>", "Saved from <realm>").

The player never sees a local GUID, a character or session UUID, a projection manifest, a progression pin, an RA command, a job file, a baseline command or a checkpoint number.
Technical detail is available under *Technical details* of an error and in the diagnostics report.

### States

| State | Meaning |
|---|---|
| Ready | The realm's copy is current; nothing is running |
| Preparing character | Play was pressed; the import / update / arm is in progress |
| Waiting for game login | The session is armed; the next login with this character is tracked |
| Playing / Syncing | A session is open; Syncing while checkpoints are in flight |
| Saving progress | The character left the game; the final state is being saved |
| Offline | The realm (or its database) is not reachable |
| Compatibility warning | The realm plays the character with limits (see section 3) |
| Update required | The realm's copy is behind and the realm has to be stopped to be updated |
| Conflict requires action | The realm's copy and the saved character both changed (section 5) |
| Incompatible | The realm cannot take the character; nothing was changed |

## 2. Layers

```
React (PortablePage, usePortable)           observes PortableState every ~1.2 s, calls portable_* commands
Tauri commands (src-tauri/src/lib.rs)       portable_state / preflight / play / make / resolve / history / add_realm / remove_realm / diagnostics / launch
PortableRuntime (service/runtime.rs)        worker thread; 1 s tick; installs refreshed every 5 s; publishes a state snapshot; call() runs one action on the thread
PortableService (service/engine.rs)         synchronous orchestration over Owner store, Host store, realm access, ServerControl
service/access.rs                           RealmAccess: how to reach a realm (installed from the registry, prepared from a descriptor)
service/view.rs                             everything the UI sees; no identifiers of the engines
engines                                     unchanged: store, realm::{import,update,reconcile,project}, session::{Owner,Host,LiveBridge}, projection
```

* The runtime is started with the application (`start_portable`) and stopped on `RunEvent::Exit`; it is independent of the screen, so navigating away changes nothing.
* The Owner and Host stores live under `<data dir>/portable/{owner,host}`. A restart resumes sessions, the outbox and the progression pins from them; `HostMemory` keeps the
  Host's in-memory throttling between ticks. The first tick after a restart checkpoints at once.
* One operation at a time (`busy`); status derivation is a pure function of stores + observations (`copy_status`).
* `COA_MANAGER_DATA_DIR` overrides the data folder (development and tests only).

## 3. Compatibility before anything is written

`preflight(character, realm)` reads the realm (capabilities, cap, running state) and evaluates the Phase 7 report for the operations that Play would do. It writes nothing.

| Verdict | Play | Shown |
|---|---|---|
| Compatible | yes | "The character is already on this realm and up to date" / "The Manager will put the character on this realm" |
| Degraded | yes | what stays safely in the canonical character; for a level cap: "This realm has level cap 60. Your level-80 character will play here as level 60. Your full level-80 progression is preserved." |
| Incompatible | no, refused before any mutation | the reasons |

The next step is `Prepare` (not on the realm), `Arm` (on the realm and current), `Resume` (a session is already armed or open), `Restart` (behind, or the realm's progression
changed: the realm has to be stopped), `Resolve` (conflict), `Offline`, `Blocked`. The projection itself is decided by the core and applied by the Manager exactly as in Phase 8;
there is no manual projection step. After `make portable` the Manager records the realm's *native* progression pin for the copy, so the realm's own character is current
and Play needs no restart.

## 4. Realms

* **Installed** (a server this Manager runs): derived from the registry/repack settings; id `srv-<install id>`; the Manager can stop and start it, so an *update* of the
  realm's copy is done automatically (`ServerControl`: stop, database up, update with the next session armed in the same transaction, start).
* **Prepared** (somebody else's / a development realm): a JSON descriptor, schema 1, `deny_unknown_fields`, in `<data dir>/portable/realms/<id>.json` (added with *Add realm file...*).

```json
{ "schema": 1, "id": "friends-realm", "name": "Friends' realm", "address": "192.168.1.20",
  "mysql_bin": "C:/path/to/mysql/bin", "db_port": 3306, "db_user": "...", "db_password": "...",
  "ra_port": 3443, "ra_user": "...", "ra_password": "...",
  "job_dir": "C:/path/to/realm/job-dir", "data_dir": "C:/path/to/realm/Data", "game_server_users": ["acore"] }
```

The descriptor holds credentials; it is trusted-local (same machine / LAN) material, never printed (`Redacted`), never part of diagnostics. An *update* of a prepared realm's copy is
not automated (the Manager cannot stop it): Play says *Update required* and the realm's owner restarts it.

## 5. Conflicts

If the realm's copy changed outside the Manager while the saved character also moved, `preview_update` reports the conflicting fields and the card shows *Conflict requires action*.
There is no automatic merge. Allowed answers: **Use the saved character** (installed realms; the realm's copy is overwritten with the canonical one, in a stopped-realm update),
**Detach the realm copy** (the Manager stops managing it; the realm's character is left exactly as it is), **Cancel**.

## 6. Diagnostics

*Copy diagnostics* produces a redacted JSON report: Manager version, per realm: id, kind, online, capability profile hash, core commit, level cap, progression signature, session
protocol; per character: id, canonical revision; per copy: realm, synced revision, session id and state, last checkpoint, progression pin, projected flag, compatibility result;
the recent errors. No credentials, no snapshots, no inventory.

## 7. Verification

* Unit tests: descriptor round trip / refusals / secrets never printed, installed realm derivation, history notes never expose identifiers.
* Live (`#[ignore]`d, real worldserver, real disposable databases, a bot standing in for the game client):
  `service::live_service::one_click_play_of_a_level_eighty_on_a_cap_sixty_realm_survives_navigation_a_restart_and_the_logout` (offline refusal, preflight, one-click Play with
  projection 80 -> 60, Host restart mid-session, final sync, history, diagnostics redaction, incompatible realm blocked with nothing written) and
  `a_character_made_portable_on_a_running_realm_is_armed_on_it_without_a_restart`.
* The Tauri window was driven over WebView2's DevTools protocol on a fresh data folder: add descriptors, make portable, preflight with the projection warning, Play, status chips,
  navigation away and back, a Manager restart in the middle of a session, logout and the saved version, history wording.
* Not covered by automation: the real game client (needs the owner's run, see the report).
