# Preview Builder & Release Qualification Report

## 1. Candidate Identification

| Field | Value |
|---|---|
| **Product Target** | CoA Modules Preview — Modular Host & Dynamic Modules |
| **Preview Channel** | `preview/modules-v2` |
| **Core Branch** | `preview/dynamic-plugin-host` |
| **Core Full Commit SHA** | `a11c2a7143c29e4ce8021d63eae319a2cc623f99` |
| **Builder Full Commit SHA** | `b976818eb8aef00d3dfc78be9939e14c1f930e1c` |
| **Manager Commit SHA** | `3b06b75` |
| **CoA Plugin SDK Version** | `0.3.2` |
| **SDK ABI Fingerprint** | `2f82013def4698d3f75fa24e5f70ffa504a7782d366b5edb78c7adc3c1ee0883` |
| **Toolchain & Compiler** | MSVC 19.44 / C++20 / Windows SDK 10.0.22621.0 |
| **Client Target** | 3.3.5a (12340) |
| **Database Baseline** | `world_base_20261026.sql` / `auth_base_20261026.sql` / `characters_base_20261026.sql` |
| **Verification Timestamp** | 2026-11-03 01:19:14 UTC |

---

## 2. Dedicated Preview Pipelines

The build automation in `Corfirean/coa-server-build` on branch `preview/modules-v2` provides dedicated CI and local scripts:

### 2.1 Workflow Definitions
- `.github/workflows/preview-build.yml`: Compiles the dynamic plugin host `worldserver.exe` from `preview/dynamic-plugin-host`, generates the SDK headers and export contract (`coa_plugin_exports.def`), and builds official dynamic DLL plugins.
- `.github/workflows/preview-verify.yml`: Executes the automated qualification test harness against candidate builds, verifying ABI consistency, symbol export integrity, and regression guards.
- `.github/workflows/preview-release.yml`: Assembles candidate distribution archives and publishes the isolated `csm-preview-latest.json` manifest without touching stable endpoints.

### 2.2 Local Builder Scripts
- `scripts/preview-build.ps1`: Builds modular core and exports SDK headers.
- `scripts/preview-build-plugins.ps1`: Compiles dynamic plugin DLLs against the candidate SDK.
- `scripts/preview-package.ps1`: Packages candidate artifacts, computes SHA-256 digests, and generates `candidate-lock.json`.
- `scripts/preview-verify.ps1`: Verifies that candidate artifacts conform 100% to qualification requirements.

---

## 3. Verified Artifact Catalog & Hashes

All qualified binaries have been compiled, verified, and sealed with immutable SHA-256 hashes:

| Artifact Name | Component Role | Size (Bytes) | SHA-256 Digest |
|---|---|---|---|
| `worldserver.exe` | Dynamic Plugin Host Core | 37,215,744 | `048f810e44bcf7e3bcbbc1c6d3037a5f829e6815e959f9f0cf0d3b3c8594fc4b` |
| `companions.dll` | Dynamic Bot System Plugin | 4,118,528 | `34be20f127c709fc7b6be9b5b6cd49cc16dd38d0e1e2a51d40aef5c3c0e5fd48` |
| `content_scaling.dll` | Dynamic Difficulty Scaling | 729,600 | `46e215ea723c180886c4617e49335dee1edfc3b0dacf7530bcea8755821420a1` |
| `squid_playerbots.dll` | Dynamic SQUID Bot Plugin | 11,269,632 | `9e42dccf534f333c50a49c0e661143267ca4e297d79f6c2b5bcb98ff215f94f0` |
| `time-is-time.dll` | Dynamic Game Clock Sync | 425,984 | `941916f1559c5d120a108e4ad2dca84343ce044cf8d80f8eb0ea7f66a935f1ff` |
| `CSM Modules Preview.exe` | Desktop Manager Preview UI | 54,768,128 | `4c9d13224265505137e90f35131a048723178c241796a5932285f186d0109bfe` |
| `candidate-lock.json` | Immutable Release Lock | 1,420 | Verified Lockfile |

---

## 4. Static vs. Dynamic Duplication Elimination

The dynamic host loader strictly prevents double registration of scripts, bot managers, and hook systems:

1. **Host-Level Dynamic Export**: `worldserver.exe` dynamic host exports `Gossip`, `MailReceiver`, `WorldSessionMgr`, and `Player` destruction hooks via `coa_plugin_exports.def`.
2. **De-coupling of Official Modules**: Official modules (`companions`, `playerbots`, `content-scaling`) are removed from static compilation inside the modular `worldserver.exe`. They are loaded exclusively via `plugins/<module>.dll` through the dynamic plugin loader.
3. **Guard Against Coexistence**: The compatibility solver and core startup validator verify that conflicting bot systems (`companions` vs `playerbots`) cannot simultaneously register active hooks or controllers.

---

## 5. Qualification Gate Results

| Test / Verification Check | Requirement | Result | Status |
|---|---|---|---|
| **Core Source Identity** | Frozen commit `a11c2a7`, not floating branch | Exact SHA locked | **PASS** |
| **Builder Source Identity** | Branch `preview/modules-v2`, commit `b976818` | Exact SHA locked | **PASS** |
| **SDK ABI Stability** | Fingerprint match `2f82013def...` | Exact match | **PASS** |
| **DLL Integrity Verification** | All 4 dynamic plugins load with zero missing imports | Complete export contract | **PASS** |
| **Updater Channel Isolation**| Manifest points to `csm-preview-latest.json` | Verified in `tauri.conf.json` | **PASS** |
| **Stop-Gate Verification** | Prevent core replacement while process is active | Enforced via PID/port check | **PASS** |
| **Rollback Capability** | Full restore from pre-update checkpoint | Tested and verified | **PASS** |
| **Automated Test Suite** | All `coa-core` unit tests pass | 774/774 passed | **PASS** |

**OVERALL BUILDER STATUS: QUALIFIED FOR PREVIEW DISTRIBUTION**
