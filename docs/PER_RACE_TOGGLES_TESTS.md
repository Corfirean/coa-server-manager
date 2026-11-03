# Per-race toggles: internal verification record

Date: 2026-10-27. Verdict: **PARTIAL**. Scope: source implementation, native builds and offline verification; live acceptance remains pending.

Latest source implementation checks additionally include:

- Manager catalog/configuration tests: 10 passed, including catalog binding, unknown-ID review, stale/invalid input rejection,
  Allow all, preserving unknown configuration keys and leaving bot exclusions unchanged.
- Focused Manager race regression selection: 34 passed, 3 ignored because they require release
  archives or another writable volume. The three ignored tests are not counted as passes.
- Frontend TypeScript/Vite build: passed after correcting the new component's error rendering.
- Existing i18n and module-settings checks: passed; new keys exist in all six languages.
- Core production-module harness: passed with synthetic Config/DBC stubs through verify_all.
- Companions production selector harness and five offline-factory policy tests: passed without
  running a MySQL client or server.
- Complete native core build with staged Companions and SQUID: passed in the dedicated
  `build-utf8` directory. Final source/build/harness verification passed in
  `C:/games/source/per-race-core/.cache/verify-all/20261027-112640/report.json`.
  Unit and gameplay stages were skipped. No installed binaries or services were changed.
- Rust Clippy with warnings denied: passed for coa-core.
- Final Tauri debug application build: passed after the catalog-review changes. Artifact:
  `C:/games/source/per-race-manager/target/debug/coa-server-manager.exe`.
  Frontend bundle-size and localized linker-output warnings were emitted; neither failed the build.

The first native configure lacked an OpenSSL root. A subsequent build was deliberately stopped
when its MSVC dependency prefix was found misencoded. The replacement uses the existing dynamic
OpenSSL/MySQL development libraries and a fresh UTF-8 environment. These earlier verify_all runs
ended FAILED and are not successful build evidence.

## Executed automated checks

Run from the Manager feature checkout:

```powershell
$env:COA_TEST_STORMLIB = '<absolute path to an existing Unicode StormLib DLL>'
python -B -m unittest discover -s tools -p 'test_race_availability_*.py' -v
```

The recorded run used the bundled repack Python at
`C:/games/CoA Server 2/Runtime/python/python.exe` and the existing DLL at
`C:/Users/Dion/AppData/Local/Programs/Python/Python311/Lib/site-packages/pympq/StormLib.dll`.
No database or game process is used. Without the explicit DLL environment variable, MPQ tests
are skipped; that is not equivalent to the recorded full pass.

| Group | Result | Actual scope |
| --- | --- | --- |
| DBC and fingerprint | PASS, 17 tests | Exact changed bytes; 1/5/50 IDs; other bits; original restoration; duplicate normalization; stock/unknown IDs; duplicate/invalid table IDs; corrupt/truncated/unknown layouts; localized name bounds and UTF-8; source/profile/set fingerprint inputs |
| Synthetic MPQ | PASS, 8 tests | 1/50 IDs produce one archive; one game payload; extraction equality; identical regenerated DBC/fingerprint; source unchanged; existing output preserved; packaging/readback/read/input errors publish no result; empty set preserves bytes |
| Total | PASS, 25 tests, zero skipped | Offline files only |
| Real three-race DBC/MPQ | PASS | Sethrak 82, Gnoll 83, Saberon 97; only Flags 12 → 13; one MPQ; re-extracted DBC equals expected; original MPQ hash unchanged |
| Existing filter behavior | CONFIRMED, 9 offline assertions | Original extracted Lua functions under Lua 5.1 with UI/CVar mocks; see existing-filter research |
| English name/ID map vs original DBC | PASS, 73 rows with NOT_PLAYABLE clear | Every inspected ID appears in the final literal Lua name map; runtime button identity remains unverified |
| Managed disabled-row Lua probe | PASS, 11 tests | Lua 5.1 syntax; grey/unchecked/disabled rows; personal choices; scroll reuse; unknown identity; Check All; click guard; six languages; input rejection; layout fallback; lifting restrictions |
| First Lua visibility probe | USER-OBSERVED | Owner reported Sethrak/Gnoll/Saberon absent and supplied screenshots; creation, login and grey rows not tested |

An initial new MPQ assertion expected one block-table entry and failed for both cardinalities.
Inspection showed StormLib also writes `(listfile)`. The corrected check requires two entries
and verifies the listfile names exactly one game file. All 25 tests then passed. This was a test
assumption correction, not a change to the functional requirement of one overlay.

Local evidence for real game files stays outside Git:
`C:/games/source/per-race-evidence/multiple-races-offline/report.json`.
Original DBC hash: `b78635b08bf0bd63d2898fce97c1fb162c8a9c41632703172eefa88bae79061f`.
Generated three-race DBC hash: `243fb36837b76ebd6943da574cc095be548b4530be7f06487debea6703db10d3`.

## Unexecuted checks

| Scope | Status | Reason |
| --- | --- | --- |
| Effective running-client DBC and general MPQ load order | NOT RUN | First Lua probe visibly affected this one installation, but effective DBC/general precedence remain unverified |
| LIVE-01 one disabled race | NOT RUN | Requires client and isolated server |
| LIVE-02 multiple disabled races and re-enable | NOT RUN | Requires client and isolated server |
| LIVE-03 existing character data/login | NOT RUN | No database/login testing performed |
| LIVE-04 bot generation, both systems separately | NOT RUN | Other bot tests are in progress; MySQL must remain untouched |
| LIVE-05 Portable Characters | NOT RUN | Requires disposable realm/database setup |
| LIVE-06 client repair/update | NOT RUN | Runtime overlay integration not implemented |
| LIVE-07 shared client | NOT RUN | Runtime profile reconciliation not implemented |
| Server config/parser/hook tests | PASS, offline | Actual module compiled with Config/DBC/crypto stubs; native build also passed |
| Client transaction crash/recovery matrix | NOT RUN | Offline result staging is not client install/recovery |
| Manager visual interaction | NOT RUN | Owner requested no screen capture/control; translation checks and frontend build passed |
| Full Rust workspace tests | NOT RUN | Focused race tests and native Manager build used instead |
| C++ build and verify_all | PASS, source/build/harness | No server/database process started; gameplay excluded |
| Release packaging and upgrades | NOT RUN | No release or runtime implementation prepared |

The copied client initially displayed a missing-critical-files message and crashed on exit.
The fault address maps to AMD's AMDXN32.DLL; root cause is unresolved. No experimental overlay
was installed at the time of that crash. Later Lua probes were installed only in the isolated copy,
and the owner confirmed hidden races and disabled grey rows. Creation could not be checked.
Neither observation is an availability live-test pass.

Existing character data and original game archives were not changed. No SQL was executed.
The feature must not be declared READY on these results.
