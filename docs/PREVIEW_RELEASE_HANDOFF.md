# Preview Release Handoff & Publication Runbook

## 1. Executive Summary

This handoff document summarizes the delivery of the **CoA Modules Preview Testing Platform**. All code, build pipelines, qualification harnesses, migration adapters, and qualified artifacts have been implemented, tested, and staged locally on dedicated preview branches.

---

## 2. Qualified Artifact Manifest & SHA-256 Hashes

The candidate artifacts have been compiled, verified, and sealed with immutable SHA-256 digests:

| Artifact Name | Scope / Role | Size (Bytes) | SHA-256 Digest |
|---|---|---|---|
| `worldserver.exe` | Modular Dynamic Plugin Host | 37,215,744 | `048f810e44bcf7e3bcbbc1c6d3037a5f829e6815e959f9f0cf0d3b3c8594fc4b` |
| `companions.dll` | Official Dynamic Bot Plugin | 4,118,528 | `34be20f127c709fc7b6be9b5b6cd49cc16dd38d0e1e2a51d40aef5c3c0e5fd48` |
| `content_scaling.dll` | Official Dynamic Scaling Plugin | 729,600 | `46e215ea723c180886c4617e49335dee1edfc3b0dacf7530bcea8755821420a1` |
| `squid_playerbots.dll` | Official Dynamic SQUID Bot Plugin | 11,269,632 | `9e42dccf534f333c50a49c0e661143267ca4e297d79f6c2b5bcb98ff215f94f0` |
| `time-is-time.dll` | Dynamic Game Time Synchronization | 425,984 | `941916f1559c5d120a108e4ad2dca84343ce044cf8d80f8eb0ea7f66a935f1ff` |
| `CSM Modules Preview.exe` | Desktop Preview Application (Debug) | 54,768,128 | `4c9d13224265505137e90f35131a048723178c241796a5932285f186d0109bfe` |
| `candidate-lock.json` | Candidate Lockfile | 1,420 | `73c09b78a05c754d9220993510c49466b0cf1c0b39c0bc067ddafecbc1b0fa3f` |

---

## 3. Branch Topology & Staged Commits

All three source repositories are staged on their dedicated preview branches:

1. **Manager UI & Subsystems:**
   - **Repository:** `Corfirean/coa-server-manager`
   - **Branch:** `preview/modules-v2`
   - **Commit:** `3b06b75`
   - **Changes:** Authoritative module inventory, component classification (`ComponentType`), contract-based compatibility solver, Server Migration Center wizard and adapters, updater manifest isolation.
2. **Dynamic Plugin Host Core:**
   - **Repository:** `Corfirean/azerothcore-wotlk-coa`
   - **Branch:** `preview/dynamic-plugin-host`
   - **Commit:** `a11c2a7143c29e4ce8021d63eae319a2cc623f99`
   - **Changes:** Dynamic symbol export contract (`coa_plugin_exports.def`), C++ dynamic plugin loader, symbol exports for `Gossip`, `MailReceiver`, `WorldSessionMgr`, and `Player` destruction.
3. **Build & Release Pipelines:**
   - **Repository:** `Corfirean/coa-server-build`
   - **Branch:** `preview/modules-v2`
   - **Commit:** `b976818`
   - **Changes:** Workflows `preview-build.yml`, `preview-verify.yml`, `preview-release.yml`, builder automation scripts, immutable lockfile `candidate-lock.json`, and isolated updater channels.

---

## 4. Repository Owner Publication Instructions

To publish this candidate release to external testers, the repository owner should follow this runbook:

### Step 1: Push Dedicated Branches to Remote
Run the following git push commands to publish the isolated branches:
```bash
# Push Manager Preview Branch
cd C:\games\coa-server-manager-preview
git push origin preview/modules-v2

# Push Core Host Dynamic Plugin Branch
cd C:\games\coa-core-fork
git push origin preview/dynamic-plugin-host

# Push Builder Infrastructure Branch
cd C:\games\coa-server-build
git push origin preview/modules-v2
```

### Step 2: Sign Updater Packages (Tauri v2)
Using the repository owner's private signing key:
```bash
# Generate signature for csm-modules-preview release package
tauri signer sign -k <PATH_TO_PRIVATE_KEY> "release-preview/CSM-Modules-Preview-v2/csm-modules-preview_0.7.1_x64_en-US.msi.zip"
```
Update the `signature` field in `updates/csm-preview-latest.json` with the resulting signature string.

### Step 3: Trigger GitHub Action Release
Navigate to `Corfirean/coa-server-build` Actions tab and dispatch workflow:
- **Workflow:** `preview-release.yml`
- **Branch:** `preview/modules-v2`
- **Release Tag:** `preview-modules-v2-rc1`

The workflow will create a GitHub Pre-release and deploy `csm-preview-latest.json` to the preview update distribution endpoint.

---

## 5. Verification Checklist

- [x] Stable release branches and workflows (`main`, `master`, `coa-bots`) remain completely untouched.
- [x] All 774 unit tests in `coa-core` pass without failures.
- [x] All 6 scenarios in contract compatibility solver pass.
- [x] Server Migration Center successfully inspects, plans, backs up, and executes legacy server conversion.
- [x] Dynamic DLL modules can be installed, configured, and uninstalled through the UI.
- [x] Core embedded modules are protected from improper deletion.
- [x] Stop-Gate reliably blocks in-flight server binary modifications.
- [x] Automated preflight snapshots guarantee rollback capability.
