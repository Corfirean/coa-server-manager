# CoA Server Manager

Windows desktop manager for local Conquest of Azeroth (AzerothCore) servers. Tauri 2 + Rust + React.
Design and audit: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Status
| Phase | State |
|-------|-------|
| 0 Audit + architecture | done |
| 1 Foundation (fsx, registry, manifest, logging, error catalogue) | done |
| 2 Read-only import, process observation, Start/Stop via repack launcher, health diagnosis | done |
| 3 Config engine + Bots/Server settings UI + presets + snapshots | done |
| 4 Recovery points, DB dump/restore (staging + atomic swap), Backups UI | done |
| 5 Clean install: signed split packages, resumable verified download, staging + atomic commit, DB credential rotation, RA account creation | done |
| 6 Updates: tracked SQL migrations, update transaction (journal, conflicts, rollback), Settings > Server updates | done |
| 7 Companions: real population, hardware-aware sizes, RA spawn (install/update of the module rides on the update flow) | done |
| 8 Client: read-only detection, backed-up realmlist, isolated addon install, START & PLAY | done |
| Backlog: interface languages EN/RU/DE/FR/ES (see docs/ARCHITECTURE.md section 9) | planned |
| 9 Friends, 10 Polish; `coa-server-build` repo (nightly sync, releases) | not started |

## Develop
```
npm install
cargo test --workspace                 # core logic (scan, fsx, manifest, process, driver, health)
npx vite                               # UI in a browser uses a mock IPC (src/dev/mock.ts)
cargo build -p coa-server-manager      # native app (debug build loads http://localhost:1420)
```
Live checks against a real repack: `cargo run -p coa-core --example scan -- <folder>` (read-only).
Start/stop testing must use a disposable copy: `tools/make-fixture.ps1` builds `C:\games\coa-fixture`
on offset ports from the pristine packaged database. Never run start/stop against a server in use.
`tests/e2e/cdp.mjs` drives the real window through WebView2 remote debugging
(`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`).
