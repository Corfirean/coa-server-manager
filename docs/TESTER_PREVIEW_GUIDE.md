# Tester Guide: CoA Modules Preview Testing Platform

Welcome to the **CoA Modules Preview**! This platform introduces dynamic C++ DLL plugins, an authoritative dynamic module inventory, independent update streams, and the automated Server Migration Center.

This guide helps external testers set up test environments, run migrations, evaluate dynamic modules, and report diagnostic findings.

---

## 1. Quick Start & Installation

### 1.1 Portable vs. Installed Mode
- **Portable Distribution:** Unpack `CSM-Modules-Preview-Portable.zip` into any folder (e.g. `C:\Games\CSM-Preview\`) and launch `CSM Modules Preview.exe`. All application state is stored locally within the directory.
- **Side-by-Side Coexistence:** The Preview application uses the unique application identifier `com.corfirean.csm.preview` and stores configuration in `%APPDATA%\CoA Server Manager Preview`. You can safely run both the **Stable CSM** and **Preview CSM** on the same Windows machine without configuration collisions.

---

## 2. Dynamic Module Management

### 2.1 The Authoritative Inventory
Unlike earlier versions that treated leftover `.conf` files as proof of installation, the Preview application verifies actual runtime presence:
- **Installed Tab:** Shows only components truly installed in the active realm (`DYNAMIC_DLL`, `CORE_EMBEDDED`, `CONTENT_PACK`, `CLIENT_PATCH`).
- **Catalog Tab:** Lists available modules ready for installation.

### 2.2 Installing a Dynamic Module
1. Open **Modules** from the left navigation bar.
2. Switch to the **Catalog** tab.
3. Choose a module (e.g. **CoA Companions**, **Content Scaling**, or **TimeIsTime**).
4. Click **Install Module**.
5. Once installed, the module moves to the **Installed** tab with a blue `DLL` badge.
6. Click the gear icon to customize module settings (e.g. companion limits, XP rates, or bot behaviors).

### 2.3 Uninstalling a Dynamic Module
1. In the **Installed** tab, locate the dynamic module.
2. Click **Remove Module**.
3. Confirm uninstallation.
4. The module DLL is removed from `plugins/`, unregistered from `.manager/plugins.json`, and immediately disappears from the **Installed** list while remaining accessible in **Catalog**.

> [!NOTE]
> Core embedded legacy modules display an amber `EMBEDDED` badge. In accordance with safety rules, embedded code cannot be removed via DLL deletion.

---

## 3. Testing the Server Migration Center

The Server Migration Center allows you to upgrade an existing monolithic AzerothCore or `coa-bots` server into the modular architecture:

### 3.1 Migration Steps
1. Click **Server Migration** (`Перенос сервера`) in the left navigation sidebar.
2. **Step 1: Source & Target**:
   - Enter or browse to the path of your legacy server (e.g. `C:\OldServer\`).
   - Select the target modular realm where you want to migrate data.
   - Click **Inspect Server**.
3. **Step 2: Source Inspection**:
   - Review detected components, core commit, database schemas, and existing module configurations.
4. **Step 3: Migration Scope**:
   - Check the components you wish to migrate:
     - [x] Module Configuration (converts legacy configs via versioned adapters)
     - [x] Character & Account Databases (preserves GUIDs, bots, items, gold)
     - [x] World Database Overrides & Custom Spawns
     - [x] Client MPQ Patches
5. **Step 4: Preflight Plan**:
   - Inspect the adapter execution plan (`CompanionsAdapter`, `ContentScalingAdapter`, etc.).
   - Verify that all conflict warnings are resolved.
6. **Step 5: Execute & Verify**:
   - Click **Execute Migration**.
   - The engine automatically creates a pre-migration safety backup in `.manager/backups/`.
   - Monitor the real-time progress bar.
   - Upon completion, review the final validation report and launch your updated modular realm!

---

## 4. Independent Update Testing

The Preview Manager features its own dedicated update channel:
- **Application Updates:** Preview checks `csm-preview-latest.json`. It will never prompt to download stable releases, nor will stable CSM download preview releases.
- **Stop-Gate Testing:** Verify that trying to update the core host while `worldserver.exe` is running correctly displays the Stop-Gate protection error.
- **Rollback Verification:** If a module update fails or the server process crashes on boot, verify that the previous working binaries can be restored from the **Backups** tab.

---

## 5. Diagnostic Reporting & Logs

If you encounter unexpected behavior, crashes, or migration errors:
1. Locate the diagnostic logs:
   - Application Log: `%APPDATA%\CoA Server Manager Preview\logs\preview.log`
   - Server World Log: `<RealmPath>\logs\worldserver.log`
   - Migration Journal: `<RealmPath>\.manager\backups\migration.journal`
2. Create a zip archive containing the logs and relevant `.conf` files.
3. Submit your report to the testing issue tracker or Discord channel with:
   - Windows OS version
   - Source server commit / version
   - Steps to reproduce
