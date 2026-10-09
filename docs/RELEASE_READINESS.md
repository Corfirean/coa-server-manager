# CoA Server Manager — Release Readiness Report

**Release Candidate Target**: `feat/portable-characters` (Phase 15 Hardened)  
**Date**: October 9, 2026  
**Evaluation Target**: CoA Server Manager Release Candidate v0.6.7  
**Verdict**: **RELEASE READY**

---

## 1. Executive Summary

Phase 15 (Release & Security Hardening) represents the culmination of all architectural phases (Phases 0–14) of CoA Server Manager. The functional architecture is frozen and verified:
- Central Registry directory & signed advertisements (HTTPS)
- Coordinator control plane with Noise XX end-to-end encryption (WSS)
- Automatic account provisioning and credential linkage
- Portable characters with remote transfers, collection synchronization, and dynamic level projection
- Direct ingress via OS-routed UPnP / NAT-PMP port mapping
- Game Relay dynamic TCP tunneling (`services/coa-relay`) with automatic fallback
- One-click Ascension client launch and crash-safe `realmlist.wtf` journaling
- Server browser integrated into Play with Friends UX

All P0 and P1 security, correctness, and reliability findings identified during the pre-release audit have been resolved, audited, and proven by automated test suites.

---

## 2. Platform & Compatibility Specifications

| Specification | Target Baseline |
|---|---|
| **Supported OS** | Windows 10 (1809+) & Windows 11 (64-bit x86_64) |
| **Supported Core Range** | AzerothCore rev. `b47c617`+ (Conquest of Azeroth build with Wildcard support) |
| **Database Engines** | Bundled MySQL 8.x / MariaDB 10.x (`acore_characters`, `acore_world`, `acore_auth`) |
| **Storage Security** | Windows Data Protection API (DPAPI `CryptProtectData`) with domain entropy separation |
| **Protocol Versions** | Registry v2, Control Protocol v1, Relay Protocol v1, Portable Character Format v1 |
| **Client Support** | Ascension WoW 3.3.5 client (Conquest of Azeroth build, ~43 GB) |

---

## 3. Public Infrastructure & Endpoints

| Service | Public Endpoint | Transport & Security | Port Surface |
|---|---|---|---|
| **Registry Directory** | `https://coa-manager.duckdns.org/registry/v2/` | HTTPS (Caddy TLS, HTTP/2) | `443/tcp` |
| **Coordinator Signaling** | `wss://coa-manager.duckdns.org/coord/v1/` | WSS (Caddy reverse proxy, authenticated) | `443/tcp` |
| **Coordinator Probe** | `https://coa-manager.duckdns.org/coord/v1/probe` | HTTPS (Strict SSRF & port-bounded checker) | `443/tcp` |
| **Game Relay Host Tunnel**| `wss://coa-manager.duckdns.org/relay/v1/host` | WSS (Ed25519 challenge-response) | `443/tcp` (via Caddy) |
| **Game Relay Client Traffic**| `coa-manager.duckdns.org:40000-43999` | Dynamic TCP Port Pool (dumb byte tunnel) | `40000–43999/tcp` |

*Internal ports (PostgreSQL `5432`, Coordinator `8081`, Relay HTTP `8082`) remain strictly unexposed to the Internet on isolated Docker bridge networks.*

---

## 4. Audit Findings & Resolution Matrix

| ID | Severity | Subsystem | Description | Status & Verification |
|---|---|---|---|---|
| **P0-1** | **P0** | Windows Secret Storage | Host realm Ed25519 signing key stored as plaintext restricted file. | **RESOLVED**: Moved to `ProtectedKeyStore` using DPAPI `CryptProtectData` with application entropy. Atomic transactional migration with fail-closed semantics verified. |
| **P0-2** | **P0** | Client Routing | Crash during game session could corrupt or permanently alter `realmlist.wtf`. | **RESOLVED**: Implemented `RealmlistOverrideJournal` preserving exact original bytes with startup recovery watcher. |
| **P0-3** | **P0** | Protocol Parsers | `rewrite_realm_list_address` integer overflow and unbounded string allocations. | **RESOLVED**: Bounds checks on address string (<256 bytes) and safe non-wrapping `u16` arithmetic implemented and tested. |
| **P1-1** | **P1** | Coordinator | Potential SSRF / port scanning abuse via `/coord/v1/probe`. | **RESOLVED**: Forbidden infrastructure port blocklist, proxy source IP verification, request body limit (4KB), and IP rate limit added and verified against attack tests. |
| **P1-2** | **P1** | Log Redaction | Secret token logging in `services/coa-relay/src/hub.rs`. | **RESOLVED**: Removed `%token` logging; expanded `SENSITIVE` keywords; test proved 0 sentinel secret leakage in diagnostic packages. |
| **P1-3** | **P1** | Portable Engine | CharacterDatabase concurrency race if `WorkerThreads > 1`. | **RESOLVED**: Preflight check `worker_threads == 1` enforced; imports fail closed with actionable error if incompatible. |
| **P1-4** | **P1** | VPS Infrastructure | Lack of automated PostgreSQL backup and restore scripts. | **RESOLVED**: Created `backup-db.sh` with atomic completion marker, SHA-256 checksums, and rotation, plus `restore-db.sh`. |
| **P1-5** | **P1** | Container Hardening | Relay container privilege and capabilities verification. | **RESOLVED**: Enforced `user: 10002`, `read_only: true`, `cap_drop ALL`, `no-new-privileges`, `mem_limit 256m`. |
| **P2-1** | **P2** | Non-Windows Keyring | Linux/macOS SecretStore uses 0600 file store rather than OS keychain. | **DEFERRED (Acceptable)**: Windows-first release. 0600 file store documented for development. |
| **P2-2** | **P2** | Native Login Prefill | Client login screen does not prefill password automatically. | **DEFERRED (Acceptable)**: Ascension client binary does not support command-line password arguments; user links account once. |
| **P2-3** | **P2** | Mid-Session Migration | Live Direct TCP connection cannot migrate mid-stream to Relay. | **DEFERRED (Acceptable)**: Automatic fallback triggers seamlessly on reconnect / subsequent JOIN. |

---

## 5. Automated Verification Results

All tests executed with `--locked` dependencies and strict warnings:

- **`coa-core`**: **593 PASSED**, 0 failed, 50 ignored (real-server live tests).
- **`coa-registry-proto`**: **19 PASSED**, 0 failed.
- **`coa-control-proto`**: **20 PASSED**, 0 failed (including 6 new hostile-input & fuzz smoke tests).
- **`coa-coordinator`**: **12 PASSED**, 0 failed (including 3 new SSRF & probe attack tests).
- **`coa-registry`**: **12 PASSED**, 0 failed.
- **`coa-relay`**: **10 PASSED**, 0 failed (including memory bounds, port exhaustion, and MySQL/RA forbidden access tests).
- **`coa-control-e2e`**: **18 PASSED**, 0 failed (12 control + 6 phase 14 direct and relay tests).
- **Frontend TypeScript (`tsc --noEmit`)**: **PASSED (0 errors)**.
- **Frontend Production Bundling (`npm run build`)**: **PASSED (built in 8.06s)**.
- **Code Formatting (`cargo fmt --all -- --check`)**: **PASSED (0 diffs)**.
- **Code Linter (`cargo clippy --workspace --all-targets -- -D warnings`)**: **PASSED (0 warnings, 0 errors)**.
- **Dependency Audit (`npm audit --omit=dev`)**: **0 vulnerabilities found**.

**Total automated passing tests**: **684 tests**.

---

## 6. Live Verification Gates Performed

1. **Gate 14.1 (Real Client Outside Host LAN)**:
   - Real Ascension client joined from outside Host LAN via public browser.
   - Verified Direct Route selected (NAT-PMP / UPnP port mapping verified).
   - Real Auth authentication completed -> `CMD_REALM_LIST` rewritten to verified external WORLD endpoint (`public_ip:mapped_world_port`).
   - Real World login -> portable character entered world -> played >10 minutes with combat, movement, chat.
   - Checkpoint recorded -> logged out -> final canonical sync completed.
   - Deliberate firewall block -> fallback to Game Relay succeeded seamlessly.
2. **Gate 14.2 (Simultaneous Mixed Mode)**:
   - Proved simultaneous coexistence of direct client and relayed client on the same host realm without port collisions or MySQL mutation.
3. **Gate 15.1 (Diagnostic Bundle Secret Scrubbing)**:
   - Diagnostic export package generated with fixtures seeded with sentinel secrets (account passwords, private keys, SRP salts, tokens, portable character bodies). Verified 0 leakage.

---

## 7. Distribution Artifacts & SHA-256 Checksums

The release candidate produces standard Windows installer artifacts:

| Distribution Artifact | Type | Description |
|---|---|---|
| `CoA-Server-Manager_0.6.7_x64-setup.exe` | NSIS Installer | Standalone Windows installer with passive update support |
| `CoA-Server-Manager_0.6.7_x64.nsis.zip` | Archive | Portable binary archive for testing and verification |

*(SHA-256 checksums are generated automatically upon release packaging via GitHub Actions or local packaging pipeline).*

---

## 8. Rollback and Recovery Instructions

In the event of an operational issue after deployment:
1. **Host Manager**:
   - Downgrade to prior release binary.
   - `ProtectedKeyStore` retains backward compatibility; legacy `.key` files are left backed up prior to migration.
   - User database files (`mysql/data`) and portable SQLite stores (`control.sqlite`, `characters.sqlite`) are never modified destructively.
2. **VPS Services**:
   - `git checkout <previous_commit_tag>`
   - `docker compose -f services/coa-registry/deploy/registry.compose.yaml up -d --build`
   - In case of database corruption, execute `restore-db.sh <backup_dump_path>`.

---

## 9. Final Release Recommendation

**Verdict: RELEASE READY**

CoA Server Manager v0.6.7 meets all release quality, security, and stability gates. All critical paths are hardened against hostile input, sensitive secrets are guarded by Windows DPAPI with zero log leakage, direct connectivity is verified with graceful Relay fallback, and the entire workspace builds and passes 100% of automated tests.
