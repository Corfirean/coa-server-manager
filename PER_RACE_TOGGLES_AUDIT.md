# Per-race availability audit

Date: 2026-10-27. This is an internal implementation audit, not a supported-feature guide.

## Frozen sources

Separate `feat/per-race-toggles` branches and checkouts:

| Component | Checkout | Fetched base |
| --- | --- | --- |
| Manager | `C:/games/source/per-race-manager` | `origin/master`, `925a81f` |
| Core | `C:/games/source/per-race-core` | `origin/coa-bots`, `1c658cab46d5057a8972cebd84c840aa6fbc4a91` |
| CoA Companions | `C:/games/source/per-race-companions` | `origin/master`, `6a289fc2230ce9425924ca86dee02d0bb8d3d344` |
| Build | `C:/games/source/per-race-build` | `origin/main`, `89795b6` |

The build runbook explicitly selects `Corfirean/azerothcore-wotlk-coa:coa-bots`, not its `main`.
The old `C:/games/source/server/azerothcore-wotlk-coa` checkout instead tracks jealous-sound upstream
and has local modifications. It must not be used as the feature's release base.
The primary Manager and Companions checkouts also contain unrelated work and remain untouched.

SQUID is a separate repository, `Zyth45/mod-playerbots`, not `Corfirean/mod-coa-playerbots`.
Existing research checkout: `C:/games/source/squid-playerbots`. Its current local branch is
`fix/custom-races-existing-bots`, one commit ahead of `origin/coa`. Build policy selects the highest
stable numeric tag, so that local branch must not silently become the shipped dependency.

## Manager integration points

- `crates/coa-core/src/custom_races.rs`: package version `1.9.2`, release URLs, MPQ/root/addon lists,
  hash manifests, client status, install, enable/disable and uninstall.
- `PatchTransaction`, `SavedFile`, `recover_patch_transaction`: shared client-relative journal in
  `.coa-races-transaction/journal.json`, saved files and `committed` marker. Uncommitted changes roll back.
  Paths use `fsx::safe_join`; file moves handle cross-volume addon locations.
- `patch_lock`: exclusive OS file lock on `.coa-races.lock`. The empty file's existence is normal and
  does not prove another operation is active. Do not delete it as a stale-lock workaround.
- Client mutations reject `client::is_running`; reuse this gate and lock for availability changes.
- `patch_files`, `matches_patch`, `preserve_original`: ownership and preservation depend on manifests
  and hashes, not just a filename. Availability overlay ownership needs its own verified record.
- `modules/catalog.json`: `custom-races` uses `coa_custom_races.conf` and `CoACustomRaces.Enable`.
  Companions use `mod_coa_playerbots.conf` / `CoaBots.Enable`; SQUID uses
  `playerbots.conf` / `AiPlayerbot.Enabled`.
- `crates/coa-core/src/modules.rs`: `toggle`, `settings_in`, `save_settings_in` write current module
  configuration. Bot switches enforce stopped servers and the existing control lock.
- `src/screens/ModulesPage.tsx`: Custom Races card, master toggle, install/progress/status UI.
- `src/lib/api.ts` and `src-tauri/src/lib.rs`: existing `custom_races_status`,
  `custom_races_install`, `custom_races_uninstall`, `custom_races_toggle_client` commands.
  There is no per-race command or verified availability status yet.
- `src-tauri/src/lib.rs::play`: local launch reads `realms::state`, calls `client::sync_realm`, then
  launches. Remote launch has a separate realmlist override path; it cannot infer a remote race policy.
- `crates/coa-core/src/realms.rs`: modes CoA, Wildcard and Warcraft Reborn; current configs are captured
  under `Settings/realm-profiles/<mode>.json`. Use this mechanism rather than a parallel realm store.
  Existing bot restrictions for non-CoA modes must remain effective.
- `crates/coa-core/src/clientdl.rs::apply`: manifest-based client updates, `keep_modified`, staging,
  preservation of replaced files. No availability-overlay invalidation or rebuild exists today.

### Confirmed reconciliation risk

`custom_races.rs::sync_bot_races_config` writes a replacement `AiPlayerbot.ExcludedBotRaces` value:
recommended full exclusions or experimental `65,72,77,80,81`. It does not preserve arbitrary user
exclusions. `modules.rs` calls it from the master switch and BotsEnable saving; some callers discard
errors. Per-race integration cannot rely on this overwrite behavior.

`playerbots_conf_paths` can also copy a `.dist` file when no active config exists; it is not a purely
read-only discovery API. Status inspection should avoid adding more implicit writes.

## Core gates and bypasses

- `modules/mod-coa-custom-races/src/mod_coa_custom_races.cpp`: `WorldScript` loads the master switch;
  `AccountScript::CanAccountCreateCharacter` accepts stock IDs 1–8, 10, 11 or an enabled master switch.
  No disabled-ID list is implemented. Stock ID 9 is deliberately not included.
- `modules/mod-coa-custom-races/conf/coa_custom_races.conf.dist`: Enable and BotsEnable exist;
  DisabledRaceIds does not.
- `src/server/game/Handlers/CharacterHandler.cpp`: normal character creation invokes
  `sScriptMgr->CanAccountCreateCharacter` inside its finalization callback and rejects with
  `CHAR_CREATE_DISABLED`. A packet from an unpatched client reaches that gate too.
- `src/server/game/Scripting/ScriptDefines/AccountScript.cpp`: hook aggregation must be preserved.
- `src/server/game/DataStores/DBCStores.cpp::LoadDBCStores`: chooses `DataDir/dbc_races/<file>` if
  the master switch is enabled and that file exists, otherwise `DataDir/dbc/<file>`.
  Per-race creation policy must not remove DBC rows or change login data.
- `.agents/docs/systems/ascension-character-creation.md`: native client compatibility depends on
  `CharBaseInfo.dbc`; race button indices are not RaceIDs. Existing race/class checks must stay intact.

## Both bot systems

### CoA Companions

`module/src/BotSpawnRandom.cpp::GetValidBotRaces` reads the global and bot custom-race switches,
`CoaBots.CustomRaces.Enable`, and both AiPlayerbot/CoaBots exclusion lists. It checks DBC presence.
Its experimental defaults apply only when the combined exclusion set is empty: adding an owner list
to that set would silently remove those defaults. Keep those policies independent.

`CreateBotClone`, `CreateOneRandomBot`, and leveled spawning clone SQL character data directly;
they bypass the account creation hook. Enforce new-character policy at the common clone creation
entry as well as random selection. Preserve existing bots and faction selection via
`Player::TeamIdForRace`.

Additional confirmed callers: `module/src/BotBattlegroundFill.cpp::QueueBotForSide` and
`module/src/BotLfgFill.cpp` call `CreateOneRandomBot` after trying to reuse an existing bot.
The existing-bot reuse path must remain separate from creation restrictions.

`tools/offline_bot_factory.py` is another direct-SQL creation path. Its main loop chooses a
template by class and faction, then copies `template.race` into the new character overrides.
It currently does not read the custom-race creation policy. Reject or filter prohibited
templates before generating new accounts, GUIDs or SQL; do not rewrite their races.

### SQUID Playerbots

`src/PlayerbotAIConfig.cpp` loads `AiPlayerbot.ExcludedBotRaces` and adds
`AiPlayerbot.CrashingBotRaces`; it reads BotsEnable. `src/CoaBotRacePolicy.h` implements
`IsCoaBotRaceAllowed`. `src/Bot/Factory/RandomPlayerbotFactory.cpp::CreateRandomBot` applies
that policy before faction and race/class selection.

The same predicate also appears in `src/Bot/RandomPlayerbotMgr.cpp` and
`src/Bot/PlayerbotMgr.cpp`, including existing-character admission. Do not append owner creation
restrictions globally to this shared exclusion vector: that would also affect existing bots.
The predicate currently does not itself require `CoACustomRaces.Enable`.
`RandomPlayerbotFactory.cpp` calls `Player::Create` directly without the account hook.
Use a creation-only check in this factory, leaving existing-character admission unchanged.

## Portable Characters

Manager import code is under `crates/coa-core/src/portable/realm/`.
`import.rs` validates identity namespaces and race/class start data (`UnknownRaceClass`).
Availability policy must not become an import rejection or an identity rewrite. The inspected
creation hook is in the normal packet handler, not this importer. In the core,
`src/server/coa/CoAPortableImport.cpp` checks `GetPlayerInfo(model.Race, model.Class)` and writes
`model.Race` to the imported character and character cache. It does not invoke the account
creation hook. Preserve this separation. Live compatibility is still untested.

## Client experiment, actual observations

The owner updated `C:/games/Ascension` from 1.9.1 to 1.9.2 during this audit; its manifest now says
`# CoA Custom 1.9.2`. A separate physical client copy is being prepared under
`C:/games/source/per-race-evidence/client`; game assets are outside Git. The copy completed:
25646 files copied, zero failures. The separate server file copy also completed, but has not
been configured or started. No database service has been started for this task.

Read-only MPQ membership probing found ChrRaces in patch-ZZZF, ZZZE, ZZZC, ZZZ, ZZW, ZZV, ZZU,
ZZS, T, M and several locale archives. CharacterCreate.lua has multiple owners too.
The old `mpqprobe.py` precedence model explicitly admits an inference for Ascension multi-letter
archives. Therefore `patch-ZZZF.MPQ` is a candidate source, not runtime-confirmed effective data.
`patch-ZZZZ.MPQ` exists already; no overlay filename has been qualified.

Extracted from `patch-ZZZF.MPQ`:

- WDBC, 95 records, 69 fields, record size 276, string block 2428, total bytes 28668.
- SHA-256 `b78635b08bf0bd63d2898fce97c1fb162c8a9c41632703172eefa88bae79061f`.
- Actual rows: Sethrak 82, Gnoll 83, Saberon 97. Their Flags are 12.
- String block starts with `Hu`, not NUL; imposing an initial empty-string invariant incorrectly
  rejects this real table. String boundaries and referenced offsets still need validation.
- Sethrak experiment changes only Flags 12 → 13. All other bytes compare equal.
- One generated MPQ was read back through StormLib; its DBC equals the expected patched bytes.
- The installed `pympq` Python extension crashed during interpreter finalization. The experimental
  tool instead uses the installed Unicode StormLib DLL directly. It is not a runtime dependency
  proposal for Manager or its users.

`tools/race_availability_poc.py` requires an explicit source archive and creates output only in a new
directory. It does not select an archive order, install an overlay or assert in-game success.
`tools/test_race_availability_poc.py` uses synthetic bytes only.

### Background verification after interactive testing was deferred

The owner requested no screen capture/input and no client/server/MySQL launches while other bot
tests run. These restrictions remain active. The owner reported that the copied client launches;
this does not confirm creation behavior or effective DBC loading.

- 17 DBC/fingerprint tests and 8 synthetic MPQ integration tests pass (25 total, none skipped).
- Native tests use the explicitly selected existing Unicode StormLib DLL, with no new install.
- Output is built and verified in a temporary directory before publishing the offline result folder.
  Failures leave no published result; this is not the future client transaction implementation.
- One and fifty disabled races both produce one MPQ. StormLib adds an internal `(listfile)`;
  the sole game payload is `DBFilesClient/ChrRaces.dbc`.
- Every referenced localized race-name offset is bounds-checked; invalid UTF-8 is rejected.
- The experimental fingerprint includes original DBC SHA-256, normalized IDs, algorithm version
  and explicitly recorded profile. A profile label does not prove client compatibility.
- The real three-race offline experiment is in
  `C:/games/source/per-race-evidence/multiple-races-offline/report.json`.
  Source MPQ SHA-256: `40eac695cfcafb598b8376f3b9a61ecc037e4d445b87279cfe387a91edad32c3`.
  Generated DBC SHA-256: `243fb36837b76ebd6943da574cc095be548b4530be7f06487debea6703db10d3`.

The first copy launch showed a missing-critical-files message and subsequently crashed on exit.
The crash report identifies `AMDXN32.DLL` in the AMD graphics driver at the fault address; this
does not establish the root cause. EXE, Extensions, dinput8, d3d9, dxgi, EsteriaAppearance and
patch-ZZZF hashes matched the primary client during comparison. The experimental overlay was
not installed. No graphics-driver or client-runtime changes were made to work around the crash.

## Reference implementations and update preservation

Inspected read-only reference clones:

- `aspollon/AFK-Realm` at `ea9c5bf691c98cace8cc03c06183c161022946de`.
- `aspollon/mod-custom-races` at `5a8f2ff8a0d494ffadf64e677a438aa7f5a6671b`.

`mod-custom-races/afk-realm.json` declares client patch generation and GlueXML Lua insertion.
`client/CustomRaces.lua` extends and lays out buttons, using race names/files and configured IDs.
This is a reference, not evidence of compatibility with the current CoA/Esteria UI or a reason
to replace CSM's prebuilt-server distribution.

`crates/coa-core/src/config/merge.rs` performs additive `.dist` merging: missing keys are added,
existing user values and unknown keys are retained. A new DisabledRaceIds key should use this
path. Existing realm snapshots already capture module `.conf` files. Neither mechanism currently
coordinates an availability overlay with client update/repair; that remains implementation work.

## Open gates

Update after the owner found the existing Hide Races panel: see
[existing-filter research](docs/PER_RACE_EXISTING_FILTER_RESEARCH.md). The inspected Lua already
supports personal name/index-based hiding, but it is not a realm-scoped RaceID policy. Nine
offline Lua 5.1 probes confirmed key behavior and limitations. Reusing its layout/UI is now a
candidate to investigate before choosing a production DBC implementation. This is not evidence
that the DBC approach failed, and it does not qualify client support.

The owner subsequently confirmed the GlueXML probe: restricted races disappeared from selection
and were grey, unchecked and disabled in Hide Races. The production client integration should
reuse this filter; changing DBC flags is no longer the selected implementation path.

Server controls, strict catalog-bound configuration, the core creation hook and both bot factory
guards are implemented and have passed offline checks and native builds. Existing-bot admission
and user/crashing exclusions remain separate from the new creation policy.

Remaining gates:
1. Integrate automatic client reconciliation, ownership and recovery with realm settings.
2. Qualify client update/repair and shared-client PLAY behavior.
3. Verify live creation rejection/re-enable, existing-character login, imports and both bot systems
   when the owner's database test environment becomes available.

Status: **PARTIAL** — server implementation and builds pass; live server acceptance and production
client synchronization remain pending. See docs/PER_RACE_TOGGLES_TESTS.md for exact scope.
No existing character data, production service, release channel, or primary client archive was changed.
