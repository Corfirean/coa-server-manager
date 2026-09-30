# CoA Server Manager

Windows desktop manager for local Conquest of Azeroth (AzerothCore) servers. Tauri 2 + Rust + React.
Design and audit: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Status
| Phase | State |
|-------|-------|
| 0 Audit + architecture | done |
| 1 Foundation (fsx, registry, manifest, logging, error catalogue) | done |
| 2 Read-only import, process observation, Start/Stop via repack launcher, health diagnosis | done |
| 3 Config UI, 4 Backups, 5 Clean install, 6 Updates/CI, 7 Bots, 8 Client, 9 Friends, 10 Polish | not started |

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
