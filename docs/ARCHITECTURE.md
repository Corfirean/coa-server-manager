# CoA Server Manager — Phase 0 audit & architecture

Status: Phase 0 output. Everything under **Audit findings** was checked against the real repositories and a real
local install (`C:\games\CoA-Repack`); nothing there is invented. Everything under **Decisions** is a proposal
that the phases below implement.

---------------------------------------------------------------------------------------------------------

## 1. Audit findings (facts)

### 1.1 The repack already defines the on-disk layout

`C:\games\CoA-Repack` (RELEASE.json `mainRevision c3beca68`, `sourceRevision b8615469`, 2026-09-11):

```
Core\                  authserver.exe worldserver.exe, DLLs, configs\, configs\modules\, Logs\, Crashes\
Data\                  maps vmaps mmaps dbc (incl. dbc\Ascension) Cameras            3.8 GB
mysql\                 MySQL 8.4.9 (mysqld, mysql, mysqladmin, mysqldump), data\ (3.3 GB), data.7z (141 MB)
Runtime\python\        embedded Python used by Scripts\
Scripts\manage.py      the launcher/supervisor (604 lines) + gui_server.py (local web GUI)
Settings\              repack.json (ports, RA creds), database.json (generated DB passwords),
                       *.conf.template  (+ .original)
BugReport\             relay.py, reports\, Logs\
Source\                server-source (2.2 GB) + zip + source-manifest.json
.state\                <name>.json = {pid, exe, created(FILETIME)} for mysql/auth/world/supervisor/relay,
                       control.lock, configuration.json, debug-logs.json
RELEASE.json           {mainRevision, sourceRevision, binaries[{Name,SHA256,Bytes}], account, ...}
MANIFEST.json          {fileCount 19382, bytes 3.2 GB, files{path:{bytes,sha256}}}   <- per-file hashes
*.bat                  thin wrappers around Runtime\python\python.exe Scripts\manage.py <verb>
```

Consequences:

* **A per-file hash manifest already exists** (`MANIFEST.json`). It is the baseline for the file-ownership
  system on imported repacks: file present in MANIFEST with the same hash → `core/repack, pristine`; present with a
  different hash → `modified`; not in MANIFEST → `user`. No need to hash 3 GB to classify.
* **Generated configs are overwritten on every start.** `manage.py prepare()` renders
  `Settings\*.template` → `Core\configs\{authserver,worldserver}.conf` and
  `modules\mod_ascension_compat.conf` (placeholders `@DATA@ @LOGS@ @LoginDatabaseInfo@ ...`), also writes
  `mysql\my.ini`, `mysql\admin-client.ini` (contains the root password) and `modules\coa_bugreport.conf`.
  The README says so explicitly. **The templates, not the `.conf` files, are the source of truth** for those three
  configs. Other module confs (e.g. `mod_coa_playerbots.conf`) are *not* templated and are edited in place.
* **Process identity is already solved in the repack** the way the spec demands: `{pid, exe, creation FILETIME}`
  via `QueryFullProcessImageNameW` + `GetProcessTimes`; listeners via `GetExtendedTcpTable` with owning PID;
  a start/stop mutex via `msvcrt.locking` on `.state\control.lock` (1 byte at offset 0 — compatible with
  `LockFileEx`); graceful stop = RA `server shutdown 1s 0`, then auth terminate, then `mysqladmin shutdown`.
  `stop_all` never hard-kills worldserver. Port offset support exists (`--port-offset`).
* **World is started under a supervisor** (`watch-world`) that also runs the Python bug-report relay; starting
  `worldserver.exe` directly skips the relay. The relay ships reports to the maintainers.
* **Ports:** MySQL 3307 (bind 127.0.0.1), auth 3724, world 8085, RA 3443 (RA user/pass in `repack.json`).
* **DB credentials are the *same for every repack download*** (`database.json` is packaged, 48-hex passwords).
  Safe only because MySQL is loopback-only. New Manager-created installs must regenerate them.
* **Readiness marker** in `Core\Logs\Server.log`:
  `AzerothCore rev. <12-hex> <date> (main branch) (...) (worldserver-daemon) ready...` and
  `WORLD: World Initialized In N Minutes M Seconds`. The worldserver banner also **reports its own core commit**
  (12 hex) — usable to identify the core version of an imported binary when RELEASE.json is stale.

### 1.2 The reference machine is a real, customised, *running* install

* `Core\worldserver.exe` sha256 `b9aa8f36…` ≠ RELEASE.json (`715bd375…`): the owner's dev build. `Core\`
  holds ~60 `worldserver.exe.pre-*` backups and `.pdb`s — **157 GB**. Import must not walk-and-hash everything;
  it hashes only what it needs (exes, configs, templates, addon) and enumerates the rest by name/size.
* `mod_coa_playerbots.conf` has 71 `CoaBots.*` keys while the repo's `dist/conf/*.dist` has ~30: real configs
  are ahead of the shipped template. The bot schema must therefore tolerate unknown keys (spec §10 already
  demands it) and be validated against the *deployed* `.dist`, not only the repo's.
* The worldserver on this machine is **running now (pid 22080)** and the owner runs long bot experiments on it.
  **Testing rule: the live install is only ever read.** Start/stop/update tests run on a disposable fixture copy
  on different ports (`--port-offset`), never on `C:\games\CoA-Repack`.
* `C:` has 86 GB free (94 % full). Backup/staging code must check free space *before* writing and default
  large artefacts (staging, DB dumps) to a user-selectable location.

### 1.3 mod-coa-playerbots is not a plug-in — it is a rebuilt worldserver

`Corfirean/mod-coa-playerbots` (`master`, no GitHub Releases yet) ships `dist\`:

```
bin\worldserver.exe (not in git)   Install-CoaBots.ps1/.bat  Uninstall-CoaBots.ps1/.bat
conf\mod_coa_playerbots.conf.dist  reference\ascensionsidekick-level-builds.json (282 KB)
addon\CoABotUI\{CoABotUI.lua,.toc} sql\2026_09_15_00_ascension_character_selection.sql
release.json {builtForRepack{mainRevision, sourceRevision}, originalWorldserverSha256[], patchedWorldserverSha256[]}
```

* The module is compiled **statically into `worldserver.exe`** (`-DMODULES=static`) *and* needs three small
  **core patches** (`docs/core-patches.md`: `LoginQueryHolder` moved to `WorldSession.h`,
  `IWorld::AddQueryHolderCallback`, `Group::GetRolls`, plus an Ascension talent accessor). The doc says some of
  these were still uncommitted in the core working tree — **must be verified before CI can build bots
  from clean commits** (§6 risk R1).
* "Install bots" today = replace `Core\worldserver.exe` (backup → `.orig`), drop `.conf.dist`, create `.conf`
  only if absent, copy `Core\reference\ascensionsidekick-level-builds.json`, copy the addon to
  `<repack>\Client\Interface\AddOns\CoABotUI` (a `Client\` folder that the repack does not have — the client is
  distributed separately), apply `dist\sql\*.sql` to the characters DB with the password read out of
  `worldserver.conf` (visible on the command line).
* The installer already does **hash-gated binary replacement** (original ∈ known hashes or patched ∈ known
  hashes, else refuse unless `-Force`) and refuses if worldserver is running. The Manager generalises this.
* Bots are real characters on their own accounts (`CoaBotHost*`) in `acore_characters`; no extra database.
  Bot creation throttles (`BatchSize 5 / 500 ms`) exist because a 1000-bot synchronous spawn once killed the
  server — presets must never lower these. (Confirmed by the owner's own incident notes.)
* The `.conf.dist` is well commented prose — usable as the source for the initial `schemas/bots.json`.

### 1.4 azerothcore-wotlk-coa (core)

* `main`, 1.1 GB, **no releases, no tags, no Windows build workflow**. Only `quality.yml` (ubuntu, source
  boundaries) exists. CI for binaries must be written from scratch.
* Build: CMake ≥ out-of-source, `-DSCRIPTS=static -DMODULES=static`, C++20, Boost.PropertyTree (vcpkg
  `boost-property-tree`), OpenSSL 3, MySQL client libs. Repo rules (`AGENTS.md`): never edit
  `data/sql/base|archive|updates/db_*`; new SQL goes only in `data/sql/updates/pending_db_*/`; **automatic DB
  updater is disabled in this project's config** (the bots doc: `AUTOUPDATER: ... disabled`). So schema
  migrations are *not* applied by worldserver — the Manager must own a tracked migration runner (spec §18).
* `modules\` contains 24 in-tree modules (mod-ascension, mod-coa-*, …); bots would be the 25th.
  Runtime module configs are `Core\configs\modules\*.conf` (+ `.dist`).
* Core HEAD 442cf4c9 (2026-09-30) is ahead of the repack's `mainRevision c3beca68` and the running binary
  (`3567e2f8e9d5`): three different revisions are in play on one machine. The compatibility matrix must be
  keyed by **exact core commit**, never by "latest".

### 1.5 Toolchain on this machine

Node 26, npm 11, git 2.55, gh (authed as Corfirean), VS 18 Insiders with MSVC 14.51 and WebView2 present.
Rust was **not** installed; `rustup` stable 1.98.1 (msvc) was installed as part of this task. cmake/pnpm absent
(npm is used; CMake is only needed by the *build* repo, on the CI runner).

---------------------------------------------------------------------------------------------------------

## 2. Decisions (architecture)

### D1. One supported layout: the "CoA Repack layout v1"

Both flows converge on the same folder shape:

* **Import** adopts an existing repack-shaped folder (classification: Healthy / Partial / Unknown-custom /
  Incompatible, §5).
* **New install** downloads the *base package* (= a repack) and extracts it into staging, verifies, then moves
  it into place. Every install is therefore a repack plus Manager metadata.

Result: one code path for process control, config, DB and update. "Unknown custom server" gets the restricted
mode from spec §52 (start/stop via detected exes, backups, console, no update/bots).

A `Layout` trait isolates this (`RepackLayout` now; a hypothetical `NativeLayout` later) so nothing hardcodes
`Core\worldserver.exe`; paths come from `Installation.layout`.

### D2. Manager metadata lives *next to*, not inside, the server folder

`D:\CoA-Repack` ⇒ `D:\CoA-Repack.manager\` (the example in the spec). Rationale: import stays byte-for-byte
read-only on the server folder, folder can be copied/moved without dragging Manager state, backups get a
natural home the user can redirect to another drive.

```
<install>.manager\
  install.json          identity + detected versions + db info (no secrets) + client path
  state.json            last known process state, update-transaction status pointer
  ownership.sqlite      (or ownership.json in v1)  path → {owner, hash, version, replacePolicy, origin}
  manifests\            copies of every applied release manifest
  backups\<id>\         recovery points (each has backup.json + payload)
  migrations\           applied-migration ledger (mirrors the DB table, see D7)
  staging\              update/install staging (same volume as install → atomic rename)
  logs\ manager.log (rotated 5×2 MB) ; cache\
```

A registry at `%LOCALAPPDATA%\CoAServerManager\installs.json` maps `installationId → path`, so a *moved* folder
is re-linkable (the `.manager` folder holds the id; if the path is gone the UI offers "Locate server").
Secrets (DB/RA passwords generated by the Manager) go to **Windows DPAPI-protected** files in the same
`.manager` folder (`secrets.bin`, `CryptProtectData`, user scope). Imported servers keep their own
`Settings\database.json`; the Manager reads it in memory, never displays or logs it.

### D3. Process model (Rust, `process` module)

* `ProcessIdentity {pid, exe_path, creation_time_filetime}` — identical to `.state\*.json`, so records are
  interchangeable with the repack's own scripts.
* A process "belongs" to an installation iff `exe_path` is under the installation root (or equals a
  configured exe) **and** creation time matches a recorded identity *or* it owns a listening socket on a
  configured port. Never by image name alone; never by bare PID.
* Discovery on startup = scan running processes for the installation's exes + `GetExtendedTcpTable` owner PIDs →
  rebuild state (works after a Manager crash, PC crash → stale PID dropped because creation-time/exe mismatch).
* Start/stop are serialised with the repack's `.state\control.lock` via `LockFileEx` (same byte), so Manager and
  `.bat` files cannot race.
* **Driver strategy.** Phase 2 delegates start/stop to the repack's own `manage.py` through its bundled Python
  (`Runtime\python\python.exe -B Scripts\manage.py start-all|stop-all`), because it already handles config
  rendering, relay, RA shutdown and readiness, and because behaviour must not regress on a production install.
  Observation (identity, ports, health, logs) is native from day one. A native Rust driver for the same
  sequence replaces the delegate later, gated behind the same `ServerDriver` trait and validated against the
  delegate by the same integration tests. Startup order/health follows spec §7–8.
* States: `Stopped | Starting | Running | Stopping | Crashed | Updating | Unknown`; overall health adds
  `Ready | NeedsAttention` using: process alive → port owned by that PID → log `ready...` marker after this
  start's timestamp → no `FATAL`/`ERROR` startup lines.
* Forced kill exists only as an explicit, warned last resort after graceful stop times out.

### D4. Config engine (`config` module)

* Line-preserving parser: keeps comments, blank lines, unknown keys, line endings and encoding (UTF-8, LF/CRLF
  detected). A `set(key, value)` rewrites only that key's line; missing keys are appended under a
  `# Added by CoA Server Manager` comment. Never re-serialises the file.
* Layered targets via the layout: for templated configs the edit is applied to `Settings\<name>.template`
  (source of truth) **and** the rendered `.conf` (so the running/next state agrees without a restart of the
  renderer); for `mod_coa_playerbots.conf` edit in place.
* Save = validate against schema → write `<file>.tmp` in same dir → re-parse and compare → snapshot previous
  into `.manager\backups\config\` → `MoveFileEx(REPLACE_EXISTING)`.
* Merge (`.dist` v1 → v2): add keys present in new `.dist` but missing; never overwrite user values; keys
  removed upstream → flagged `deprecated`, kept.
* Schemas: `schemas/bots.json`, `schemas/server.json` (curated whitelist), fields exactly per spec §10
  (`key type category title description default min max options advanced restartRequired dangerous`;
  `restartRequired ∈ runtime|world|full`). Presets are separate JSON diffs shown as "will change N settings".

### D5. File ownership & manifests

Every release ships `manifest.json`:

```jsonc
{
  "schema": 1,
  "kind": "base|update|bots",
  "version": "0.4.1",
  "core":  { "commit": "<40 hex>" },
  "bots":  { "commit": "<40 hex>" | null },
  "builtAt": "2026-…Z",
  "minManagerVersion": "0.1.0",
  "requires": { "core": ["<commit>"|"range"], "dbSchema": ">=N" },
  "files": [ { "path":"Core/worldserver.exe", "sha256":"…", "size":123, "owner":"core|bots|manager",
               "policy":"replace|replace-if-pristine|merge-config|create-if-missing|never-touch" } ],
  "migrations": [ { "id":"2026_09_15_00_…", "db":"characters", "sha256":"…", "destructive":false } ],
  "signature": { "alg":"ed25519", "keyId":"…", "sig":"…" }   // verified when a public key is embedded (§54)
}
```

`ownership` table per install: `path, owner(core|bots|manager|user), sha256_at_install, installed_version,
policy, origin(imported|installed)`. Update algorithm compares **current hash vs recorded hash vs incoming
hash**: current == recorded → safe to replace; current ≠ recorded → *modified outside Manager* → prompt
(Keep / Replace / Compare / Cancel); path unknown → `user`, never touched. Import seeds the table from
`MANIFEST.json` where present (imports are read-only; hashes computed lazily and only for managed paths).

### D6. Update transaction (Prepare → Verify → Snapshot → Apply → Validate → Commit)

1. **Prepare** — fetch manifest (HTTPS only, signature checked), compatibility matrix check, disk-space check on
   staging volume and backup volume.
2. **Download** to `staging\<txid>\dl\` with `.part` files, HTTP Range resume, retry/backoff, cancel, progress.
3. **Verify** — SHA-256 of every artefact; nothing is executed or extracted into the live tree before this.
4. **Snapshot** — server must be stopped (never copy over a running worldserver); create recovery point:
   replaced binaries, changed managed configs, module state, `characters`+`auth` dump (world optional);
   write `txn.json {state: Snapshotted}`.
5. **Apply** into `staging\<txid>\tree\`, resolve ownership conflicts, merge configs, then swap file-by-file with
   same-volume atomic renames, journaling each step (`txn.json` lists done/undone operations).
6. **Migrate** — run pending SQL through the tracked runner (D7); failure ⇒ stop, keep DB backup, offer restore.
7. **Validate** — start server, health check (D3) with timeout.
8. **Commit** — ownership table + `install.json` version bump, mark txn `Committed`, prune staging.
   Any crash before Commit leaves `txn.json` in a non-terminal state; on next launch the Manager detects it and
   offers *Roll forward* or *Roll back* — the install is "recoverable" by construction.

Rollback = binaries + managed config state + module state only. Database restore is a **separate explicit
action** (spec §19–20).

### D7. Database strategy (`db` module)

* Driver: talk to the repack's own `mysql.exe` / `mysqldump.exe` / `mysqladmin.exe` (bundled, 8.4.9). Passwords
  are passed via a temp `--defaults-extra-file` with restrictive ACL, deleted immediately — never on argv. A
  native Rust client (`mysql_async`) is used for health pings and migration ledger reads.
* Ledger: table `acore_world.coa_manager_migrations(id, db, sha256, applied_at, status, error)` (created by the
  Manager, in a dedicated table — no core schema is changed) mirrored to `.manager\migrations\`.
  Statuses `Applied | Pending | Failed`. Each file runs in a single connection; DDL is not transactional, so
  "destructive" migrations (flag in manifest, or heuristic scan for `DROP|TRUNCATE|DELETE|ALTER ... DROP`) force
  a fresh DB backup first and require explicit confirmation.
* Backups: `mysqldump --single-transaction --routines --quick` per DB. Quick = characters + auth + configs +
  metadata; Full adds world (large); Config-only; DB-only. Compressed with zstd. Restore is always to a
  staging schema first (`…_restore`), swapped only after a row-count sanity check, with the current DB dumped
  beforehand.
* Never: `DROP DATABASE`, resetting characters, overwriting DBs, or restoring DB on a binary rollback. Delete/
  reset lives only in **Settings → Advanced → Danger Zone**, deletes tracked files only, and refuses to remove a
  directory that still contains untracked files.
* Clean install: extract `mysql\data.7z` (packaged data dir), then **rotate** root/app passwords to fresh random
  values (`ALTER USER`), rewrite `Settings\database.json`/templates via the layout renderer, store copies in
  DPAPI. `bind-address=127.0.0.1` enforced.

### D8. Bot install/update (needs server-side cooperation — see §4)

Compatibility matrix `compat.json` (published with releases): `[{manager, core, bots, dbSchema}]` keyed by exact
core commit. Bots install = "install the bot-enabled `worldserver.exe` built for **this** core commit" +
conf.dist + talent JSON + SQL migration + (if client attached) addon. Same hash-gated + backup-first rules as
the existing `Install-CoaBots.ps1`, executed through the D6 transaction. Force-install on unknown core =
Advanced override, snapshot first.

### D9. Client integration

The client is a separate object. Manager only: detect (`Wow.exe`/`Ascension.exe` + `Data\`), read/write
`realmlist.wtf` after backing it up, and copy `Interface\AddOns\CoABotUI` (backing that one directory up only if
its files differ from the manifest). No touching of `WTF`, `Cache`, other AddOns, or the exe.
*Reference machine note:* `C:\games\Ascension` is a heavily modified Ascension client (custom `d3d9.dll`,
renderer files, many `.bak`s) — client detection must be strictly read-only + additive.

### D10. Network / friends

Detect LAN IP and public IP (HTTPS to a small set of IP-echo endpoints, user-toggle), listening check via
outbound self-test, **CGNAT heuristic** (WAN address of router ∈ 100.64.0.0/10 or private / ≠ public IP),
UPnP/NAT-PMP later. Firewall rules created only through a separate elevated helper (UAC on demand, never whole
app as admin), fixed names `CoA Server Manager - Auth` / `- World`, idempotent (query before add), delete only
own rules. Never exposed: MySQL 3307, RA 3443, Manager control channel (Tauri IPC only, no listener). Private
network = Tailscale detection/guided enable in v1; interface `NetworkProvider` so "CoA Connect" can slot in.
Friend package contains only `realmlist`/instructions/optional addon; secrets structurally excluded (whitelist
copy, not blacklist).

### D11. Stack & code layout

```
coa-server-manager/
  docs/            ARCHITECTURE.md, flows, wireframes
  schemas/         bots.json, server.json, presets/, manifest.schema.json, compat.schema.json
  src-tauri/src/
    main.rs, commands/    thin Tauri command layer (validate → call service)
    fsx/                  path validation, safe join, atomic write/replace, disk space, ownership
    layout/               RepackLayout, detection & classification
    registry/             installs.json, .manager dir, install.json
    process/              identity, listeners, lock, drivers (delegate → native)
    health/               probes, log-marker scan, error translation
    config/               parser, merge, schema, presets
    db/                   mysql tool wrapper, ledger, migrations, dump/restore
    backup/               recovery points, retention
    update/               manifest, download, verify, txn (journal), rollback
    client/               detection, realmlist, addon
    net/                  ips, ports, cgnat, firewall, tailscale
    errors/               error catalogue → human message + actions
    diag/                 diagnostics, export package (redacting)
  src/             React + TS + Vite + Tailwind + shadcn/ui (presentation only)
  tests/           integration fixtures (fake repack generator), failure-injection harness
  coa-server-build/ (separate repo) workflows + packaging + manifest signing
```

All destructive filesystem operations exist only in `fsx`; frontend never has `fs` scope in Tauri capabilities.
Every command that mutates goes through `fsx::Transaction`-style helpers that (a) validate the path is inside a
registered installation *or* the `.manager` dir, (b) refuse symlink/junction escapes, (c) log an audit record.

### D12. Build/release repo (`coa-server-build`)

Windows runner: checkout **exact** core SHA and bots SHA from workflow inputs (refuse branch names), place bots at
`modules/mod-coa-playerbots`, verify core patches are already in the core commit (fail the build if the patch set
is not applied — never apply ad-hoc patches at build time unless they are versioned `.patch` files in the build
repo), vcpkg-cached deps, CMake Release/RelWithDebInfo static, produce **two binaries per core commit**:
`worldserver` (no bots) and `worldserver+bots`, then `authserver`, DLLs, config `.dist`s, talent JSON, addon, SQL
migrations, `RELEASE.json`, `manifest.json` (+SHA256, +ed25519 signature), zstd-compressed update package.
Base package (≈3.2 GB; Data 3.8 GB + mysql data) is split into ≤ 1.9 GB parts (GitHub release asset cap is 2 GiB)
with a parts manifest. Reproducibility: pinned runner image label, recorded compiler/vcpkg versions, source SHAs,
build timestamp written into RELEASE.json.

---------------------------------------------------------------------------------------------------------

## 3. Flows

### 3.1 First run
`Welcome → [Install new server] | [I already have a server] | (link) Manage another install`.
New: choose folder (default `C:\Games\CoA Server`; checks: free space ≥ base+backup headroom, folder empty or
absent, not a client folder, not `Documents`/system dirs, not already registered) → single progress screen
(Download → Verify → Extract → Database → Bots(optional) → Start → Health) → "Your server is ready" →
Create account → PLAY. Technical log behind "Show details" only.

### 3.2 Existing-server import (strictly read-only)
select folder → `layout::scan()` (read-only opens, `FILE_SHARE_READ|WRITE` so a live server is unaffected) →
report: core exes + hashes vs RELEASE.json/MANIFEST.json, detected core commit (RELEASE.json → else banner rev in
Server.log → else unknown), configs & modules present, DB running/reachable (ping only), bots detected (conf
present ∧ patched-hash or `CoaBots.*` keys), client detected, classification + "No files will be changed" →
**[Add server]** writes only `<folder>.manager\` and the registry entry. Import never "fixes" anything.
Verification for the fixture test: recursive (path,size,mtime) snapshot of the server folder excluding
files the running server itself changes, diffed before/after.

### 3.3 Update — see D6. Failure matrix (spec §60) is executable via a fault-injection harness: kill at step k,
disk-full at write n, network cut at byte n, migration returns error, new worldserver exits non-zero, port
taken, exe quarantined (file vanishes), folder moved, user-modified managed file, unknown config keys.

### 3.4 Rollback — restore recovery point's binaries/configs/module state; database untouched unless the user
picks "Restore database" separately (shows dump timestamp and warns that progress since then is lost).

---------------------------------------------------------------------------------------------------------

## 4. Required changes in the two source repositories

Prefer server-side cleanliness over Manager hacks. Proposed (each is an issue/PR on the owner's side, none is
made without approval):

| # | Repo | Change | Why |
|---|------|--------|-----|
| S1 | mod-coa-playerbots | Master switch `CoaBots.Enable` (0 = module registered but inert; no login/hooks/commands) | Lets a **single** worldserver ship for everyone. "Disable bots" becomes a config flip with restart, not a binary swap; drops the 2-binary matrix and most of §D8 risk. |
| S2 | mod-coa-playerbots | Commit **all** core patches in the core repo; delete "uncommitted working-tree" state from docs | CI must build from clean SHAs (R1). |
| S3 | mod-coa-playerbots | Real Releases + `dist/release.json` replaced by the shared `manifest.json` schema; `TalentBuildsPath` default = `Core/reference/…`, remove the hard-coded dev path fallback | Manager and installer share one truth; no absolute dev paths. |
| S4 | mod-coa-playerbots | Ship a machine-readable config schema (`conf/…schema.json`) next to `.dist`, or generate it in CI from the `.dist` comments | Avoid schema drift (71 vs 30 keys today). |
| S5 | core | Windows build+release workflow (or the build repo builds it); publish `worldserver --version --json` (core commit, bots commit, DB schema id) | Reliable version detection instead of log scraping. |
| S6 | core | Health endpoint alternative: worldserver already prints `ready...`; add a stable machine line (`COA-READY {...}`) or a tiny loopback status file `Core\Logs\ready.json` | Health without regex on human text. |
| S7 | core/repack | Ship a `manager-hooks.json` in the repack describing service names, ports, exes, config templates, RA settings | Layout detection without hard-coded knowledge; unknown custom builds can opt in. |
| S8 | core | Keep the auto DB updater disabled but expose `--dry-run-updates` listing pending updates | Manager migration UI without re-implementing the updater. |
| S9 | repack | Per-download random DB passwords and `Settings\database.json` generated on first start, not packaged | Security (§1.1). Manager does it itself for new installs meanwhile. |

Everything still works without S1–S9 (Manager has fallbacks), they just remove fragility.

---------------------------------------------------------------------------------------------------------

## 5. Import classification rules

* **Healthy** — `Core\{worldserver,authserver}.exe`, `Core\configs\*.conf`, `Data\dbc`, `mysql\bin\mysqld.exe` +
  `mysql\data\acore_{auth,characters,world}`, `Settings\repack.json`, `Scripts\manage.py`, all present.
* **Partial** — a repack skeleton with pieces missing (e.g. no Data\maps, DB dirs absent).
* **Unknown/custom** — has `worldserver.exe` + `authserver.exe` but not the repack scaffolding.
* **Incompatible** — not a 3.3.5 AzerothCore layout / two installations nested / client folder.
The reference install classifies as **Healthy (customised)**: repack shape, binary hash differs from RELEASE.json.

---------------------------------------------------------------------------------------------------------

## 6. Risks & open questions

* **R1 (blocking for Phase 7/6)** — are all core patches committed? Owner action: confirm/commit (S2).
* **R2** — Legal/hosting of the base package: `Data\` (maps/vmaps/mmaps/dbc) derives from the game client;
  and Ascension-specific DBC. Where does the owner want to host the 3–4 GB base (GitHub Releases in the build repo,
  parts ≤ 1.9 GB; or own storage)? The Manager's downloader is source-agnostic; the choice affects Phase 5–6.
* **R3** — Manager signing key: needs an Ed25519 key pair generated by the owner; public key embedded in the app,
  private key in GitHub Actions secrets. Phase 6 can start with unsigned + hash-only and flip the enforcement flag.
* **R4** — Bug-report relay (Python, sends reports to maintainers) is part of the repack's world startup; Manager
  keeps it (privacy note surfaced in Settings, user can disable via the existing `coa_bugreport.conf` mechanism
  only if the repack supports it — currently it forces `Enable = 1` on every start; not touched by Manager).
* **R5** — Tauri updater needs a hosted, signed `latest.json`; Manager self-update is Phase 10.
* **R6** — Windows long paths / non-ASCII install paths: repack requires a Latin path (uses 8.3 short names);
  new-install folder chooser enforces it.

---------------------------------------------------------------------------------------------------------

## 7. Phase plan (each phase ends: compile → tests → run against fixture → verify live install untouched → commit)

| Phase | Deliverable | Done when |
|-------|-------------|-----------|
| 1 | Tauri+Rust+React skeleton, `fsx`, registry, `.manager` layout, logging w/ rotation, manifest structs + JSON schema, error catalogue skeleton | `cargo test` green; app launches; metadata round-trips |
| 2 | Read-only scan/classify/import; process discovery; Start/Stop via delegate driver; health | Import of `C:\games\CoA-Repack` leaves it byte-identical (proved by snapshot diff); Start/Stop verified on **fixture copy, offset ports** |
| 3 | Config parser/merge/atomic save; bots+server schemas; presets; UI | Property tests: unknown keys/comments preserved; round-trip identity |
| 4 | Recovery points, DB dumps, config snapshots, restore-to-staging | Backup→restore fixture test; fails safe when disk full |
| 5 | Clean install from base package; DB bootstrap + password rotation; account creation | Fresh VM/folder install reaches "ready" |
| 6 | Build repo & workflows; signed manifests; update transaction, rollback | Fault-injection suite green (spec §60) |
| 7 | Bots install/update via txn; compat matrix; GUI settings | Install → disable → restore on fixture |
| 8 | Client detection, addon, realmlist, PLAY | Client tree hash unchanged except addon dir + realmlist |
| 9 | Friends: net diagnostics, firewall service, Tailscale | Idempotent rule add/remove test |
| 10 | Error translation, diagnostics, repair, a11y, self-update | Spec §61 checklist |


---------------------------------------------------------------------------------------------------------

## 8. Decisions recorded after owner review (2026-09-30)

* **Core changes go through our own fork.** We are not maintainers of `jealous-sound/azerothcore-wotlk-coa`, so
  the bot-enabling core patches (and the S1/S5/S6/S7 style changes) live in a fork owned by the bot/manager
  project; the build repo pins exact fork SHAs. Upstream is tracked as a remote and merged periodically; the
  compatibility matrix is keyed by fork commit. The fork is created when Phase 6/7 needs it (nothing is pushed
  before then). This supersedes S2 (no upstream commits required) and removes risk R1.
* **Base package is compressed and split.** Payloads are zstd/LZMA-compressed and cut into parts below the
  2 GiB GitHub asset limit (target 1.9 GB) with a parts list in the manifest; the downloader verifies each part
  and the reassembled archive. Actual ratios will be measured in Phase 5 (the packaged database already shrinks
  3.3 GB -> 141 MB; `Data\` maps/vmaps/mmaps are expected to compress far less).
* **Manifest signing (R3 resolved).** Ed25519 key generated. Public key: `keys/manifest-signing.pub`, embedded
  in the app (`signing::EMBEDDED_PUBLIC_KEY`). Private key: `%USERPROFILE%\.coa-manager\signing\` (outside the
  repo, never committed); CI receives it as the `COA_SIGNING_KEY` secret, which the owner must add manually.
  Signatures are detached (`manifest.json.sig`, over the exact manifest bytes), so the embedded `signature`
  field in the manifest was removed. **Back up the private key** — losing it means shipping a new app to rotate.


### 8.1 Fork created (2026-09-30)

* Fork: `Corfirean/azerothcore-wotlk-coa` (from `jealous-sound/azerothcore-wotlk-coa`). Default branch is `coa-bots`
  (a branch named `coa` is impossible: the fork inherits upstream's `coa/...` branches). `main` stays a pure mirror of upstream.
* `coa-bots` = upstream `main` + a small, reviewable patch series (audit of the owner's 38 local commits, 2026-09-30):
  1. `Core/Bots` hooks: `LFGMgr::GetProposalIdForPlayer`, `PlayerScript::OnPetitionOffered`, `Guild` friend `BotMgr`.
  2. Crash fixes seen under bot load: `CombatManager::PutReference`, `Unit::RemoveFromWorld` dangling delayed-visibility pointer,
     `InstanceMap::UnloadAll` teleport race, `IsStatisticAchievement` missing category.
  3. `AscensionClassServiceBridge` + `AscensionResourceQuery` (needed by the bot module) and the socketless-bot appearance-sync guard.
  4. RA: no throw on abrupt client disconnect (the Manager's RA client does exactly that).
  5. CI workflow `coa-bots-build.yml`: builds core + bots module on Windows, runs `worldserver --version`.
* Discovery: the bot chassis (`LoginQueryHolder`, `IWorld::AddQueryHolderCallback`, `Group::GetRolls`) is already in upstream
  (`feat(Core): add mod-playerbots hooks and CoA API (#3069)`), so it is not carried; `docs/core-patches.md` in the bots repo is outdated.
* Deliberately NOT carried (owner's other local work, decide separately): CoA mechanic corrections, `AscensionClassTester`
  (dev harness; couples core to the bots module), respec `SpecId==0` guard, `cs_daynight`, aura/spell null-guards in
  `SpellAuras.h`/`Spell.cpp`, class-contract data, debug `LOG_ERROR` traces in Petition/Query handlers, fleet log appenders in
  `worldserver.conf.dist`, `apps/coa-dbc` binaries, `height_query` tool.
* Sync policy: `main` fast-forwards from upstream automatically; `coa-bots` is rebased/merged onto it nightly by a workflow in the
  build repository (conflict => issue, no release). Channels: `edge` (every green nightly) and `stable` (promoted by the owner).


---------------------------------------------------------------------------------------------------------

## 9. Backlog (owner requests)

* **Interface languages: English (default), Russian, German, French, Spanish** (requested 2026-09-30).
  Plan: i18n layer in the React UI (string catalogue per locale, ICU-style plurals, locale picked from the OS with a
  switch in Settings, persisted per user); all user-facing strings currently written in English move into catalogues;
  the Rust error catalogue (`error.rs` titles/messages/actions) is keyed by `ErrorCode` and translated on the UI side
  (backend keeps stable codes; technical details stay untranslated for bug reports); dates, sizes and numbers use
  `Intl`; settings-schema titles/descriptions (`schemas/*.json`) get per-locale overlays (`schemas/i18n/<locale>.json`);
  translations are reviewed by native speakers before release, with an English fallback for missing keys and a CI check
  that every locale has every key. Do this before the first public release so strings are not retro-fitted.

  **Status (2026-09-30):** catalogue + picker + all screens translated (en/ru/de/fr/es drafts, `tools/check-i18n.mjs` checks
  keys/placeholders). Still English: settings-schema titles/descriptions/presets, companion size names, diagnostics check
  texts and update step names coming from the backend (need `schemas/i18n/<locale>.json` overlays and code-keyed backend strings).

* **Language choice on the very first screen** (requested 2026-09-30): the welcome screen must offer the language picker
  right away (before any other text is read), preselected from the OS language; the choice is persisted and shared with
  Settings. **Done 2026-09-30:** shared `LanguagePicker` (top-right of the welcome screen and in Settings).

* **Clean-install gap found by the owner (2026-09-30):** installing from the default URL fails with 404 because the `base`
  release does not exist yet (only `edge` is published). The message is now human ("package not available yet"). Also open:
  the base package has no `Data\` (maps/dbc/vmaps, ~3.8 GB); the installer needs a "use my existing Data folder / download"
  step, and the owner must decide hosting/legal for game data before `base` is published.
