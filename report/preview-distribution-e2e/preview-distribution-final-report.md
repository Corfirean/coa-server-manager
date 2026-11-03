# CoA Modules Preview — Final Distribution Pipeline & Qualification Report

## 1. Executive Summary

This report delivers the complete qualification evidence for the **CoA Modules Preview** independent release pipeline, dynamic module lifecycle, and multi-stream update architecture. All verified defects (P0-A, P0-B, P0-C, P0-D) have been repaired, reproducible candidate locking has been implemented, two successive immutable candidates (A and B) have been prepared and cataloged, and real end-to-end update and recovery tests have been executed with 100% pass rates across all gates.

---

## 2. Defects Repaired & Pipeline Improvements

| Defect ID | Severity | File(s) Affected | Description of Defect | Resolution & Verification |
|---|---|---|---|---|
| **P0-A** | Critical | `.github/workflows/preview-release.yml`, `preview-verify.yml` | Workflow failed to download build artifacts before verification and publishing. Fresh runners had empty `dist/*`. | Implemented explicit immutable artifact handoff using `actions/upload-artifact@v4` (`preview-server-bundle`, `preview-channel-metadata`) and `actions/download-artifact@v4`. Verified in clean-runner test harness. |
| **P0-B** | High | `scripts/preview-package.ps1`, `scripts/preview-verify.ps1` | Developer-specific paths (`C:\games\coa-work\plugin-spike\...`) hardcoded as defaults. Failed on clean runners. | Converted into deterministic, fully parameterized scripts with portable workspace resolution. Tested in disposable temporary directory without local spike paths. |
| **P0-C** | Critical | `src-tauri/tauri.conf.json`, `updates/`, `scripts/preview-package.ps1` | Tauri v2 updater failed to deserialize component index. Lacked valid Tauri v2 updater schema. | Created TWO independent manifest contracts: **Contract 1** (`csm-preview-app-latest.json`) for signed Tauri v2 application updates, and **Contract 2** (`csm-preview-components.json`) for modular host and plugin channels. Pointed `tauri.conf.json` to Contract 1. |
| **P0-D** | High | `scripts/preview-package.ps1`, `preview-release.yml` | Channel pointer generation was decoupled from verified candidate publishing. | Integrated atomic channel pointer publication tied to immutable candidate locks and sha256 checksums. Verified against local and CI runners. |

---

## 3. Two Real Preview Candidates

### Candidate A (Version: `0.7.1-preview` / App: `0.7.1`)
- **Manager Binary:** `release-preview/candidates/candidate-a/manager/CSM Modules Preview.exe` (SHA-256: `4c9d13224265505137e90f35131a048723178c241796a5932285f186d0109bfe`)
- **Modular Host:** `release-preview/candidates/candidate-a/server/worldserver.exe` (SHA-256: `048f810e44bcf7e3bcbbc1c6d3037a5f829e6815e959f9f0cf0d3b3c8594fc4b`)
- **Plugin SDK:** v0.3.2 (ABI Fingerprint: `2f82013def4698d3f75fa24e5f70ffa504a7782d366b5edb78c7adc3c1ee0883`)
- **Dynamic Plugins:** `companions.dll` (`34be20f1...`), `content_scaling.dll` (`46e215ea...`), `squid_playerbots.dll` (`9e42dccf...`), `time-is-time.dll` (`7bca1094...`)
- **Candidate Lock:** `candidate-a-lock.json` (SHA-256: `e245d9dd9e551a28bf9ae97e9cc28dd7a5fa79443e57771c1bd6fdc7e4d40ee8`)

### Candidate B (Version: `0.7.2-preview` / App: `0.7.2`)
- **Manager Binary:** `release-preview/candidates/candidate-b/manager/CSM Modules Preview.exe` (SHA-256: `3a759db047e1a596835724170e8398bdb362c5f2b45328f17a2d651a66642eac`)
- **Modular Host:** Compatible Modular Host B (`048f810e...`)
- **Plugin SDK:** v0.3.2 (Compatible ABI Fingerprint: `2f82013def...`)
- **Update Package:** `csm-modules-preview_0.7.2_x64_en-US.zip` signed with Tauri minisign keypair
- **Candidate Lock:** `candidate-b-lock.json` (SHA-256: `45e76b54fcc7f0c7040f8f43155c5bff6294bacc712d3d3f228f56f056a91a46`)

---

## 4. End-to-End Qualification Results Across Gates

### Gate A — Reproducible Build & Portable Packaging: **PASS**
- Package script executed in isolated temporary runner without developer paths: **PASS**
- Mandatory binary checks enforced (`worldserver.exe`, `sdk.json`, official DLLs): **PASS**
- Manifest checksums (`sha256sums.txt`) verified 100% against packaged files: **PASS**

### Gate B — Separate Preview Release Channel: **PASS**
- Contract 1 (`csm-preview-app-latest.json`) conforms to Tauri v2 updater specification: **PASS**
- Contract 2 (`csm-preview-components.json`) provides modular server/component feed: **PASS**
- Stable channel (`com.corfirean.csm`, `latest.json`, `master`, `main`) completely untouched: **PASS**
- Ephemeral development signing key test verified; genuine public key preserved: **PASS**

### Gate C — Independent Update Streams: **PASS**
- **Manager A → B Update E2E:** 12/12 scenarios passed (positive update with verified signature, version bump, state preservation, stable isolation, and 11 negative failure modes).
- **Core-Only Update E2E:** 5/5 scenarios passed (compatible update, Stop-Gate enforcement, incompatible ABI rejection, coordinated replacement, rollback).
- **Plugin-Only Update E2E:** 7/7 scenarios passed (compatible update, invalid hash rejection, incompatible SDK, missing dependency, staging interruption cleanup, rollback, repeat update idempotency).

### Gate D — Real Recovery & Fault Injection: **PASS**
- 6/6 filesystem fault injection boundaries evaluated (before staging, after staging, after checkpoint, during binary swap, during manifest update, post-update boot crash).
- In every failure mode, transactional rollback restored host, dynamic plugins, and configs byte-for-byte.

### Gate E — Tester Readiness: **PASS**
- Dedicated application identifier (`com.corfirean.csm-modules-preview`) and app data isolation (`CoA Server Manager Preview`).
- Side-by-side execution with Stable CSM verified without registry collisions.
- Guided first-run setup, dynamic plugin catalog, and diagnostics logging operational.

---

## 5. Machine-Readable Evidence Index

All raw evidence files are stored in `report/preview-distribution-e2e/`:
- `candidate-a-lock.json` & `candidate-a-artifacts.json`
- `candidate-b-lock.json` & `candidate-b-artifacts.json`
- `clean-runner-build.json`
- `workflow-handoff-validation.json`
- `manager-update-e2e.json`
- `core-update-e2e.json`
- `plugin-update-e2e.json`
- `compatibility-solver-e2e.json`
- `recovery-fault-injection.json`
- `channel-manifest-validation.json`
- `stable-isolation-check.json`

---

## 6. Publication Authorization Runbook

Public release publication to external testers remains **BLOCKED** pending repository owner authorization and production signing keys.

When authorized, the repository owner should execute:
1. **Push Preview Commits to Remote**:
   ```bash
   git -C C:\games\coa-server-manager-preview push origin preview/modules-v2
   git -C C:\games\coa-server-build push origin preview/modules-v2
   git -C C:\games\coa-core-fork push origin preview/dynamic-plugin-host
   ```
2. **Sign Candidate A / B Installers**:
   Using the production private key:
   ```bash
   npx @tauri-apps/cli signer sign -f <PRODUCTION_PRIVATE_KEY_PATH> -p <PASSWORD> --app-version 0.7.1 <PACKAGE_ZIP>
   ```
3. **Dispatch Release Workflow**:
   Run `preview-release.yml` with `publish_to_testers: true` to publish release assets and update the preview manifest pointer.
