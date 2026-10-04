# Database checks, repair and simultaneous worlds

Settings includes **Check files and database**, **Repair**, and **Start CoA and Wildcard together**.

Database checks inspect both realm schemas when Wildcard exists, the migration ledger, character-save columns and CoA starting class data. Results are saved in the installation metadata's `logs/database-checks.json` and included in exported diagnostics. Hash checks skip mutable MySQL data and configuration files.

Repair requires both worlds and auth to be stopped, no unfinished update, and the installed version to match the currently published signed update. The signed base must match the installed base as well. It verifies packages before changing files, creates a full database/config backup, saves every replaced file and a recovery inventory under `<server>.manager/repairs/<backup-id>/`, then restores official files and applies pending migrations to both worlds. Settings and player databases are not replaced. A failure during file replacement restores the original files. Interrupted repair can be retried; its file inventory and backups remain available.

Repair reports unresolved schema damage. It does not guess ALTER statements or replay already applied SQL: those cases need a new corrective migration. Applied SQL whose recorded SHA-256 changed is reported and rejected. Manager failure records take precedence over the core's old `updates` entries.

## Release schema contracts

`clean-base` clears the character migration ledger after rebuilding the character database and captures `Scripts/database-schema.json`. It contains expected table/column names and column types. Installation and migration validation inspect this contract; older packages receive the narrower character-save checks, clearly labelled in the UI.

For each update, capture a fresh contract **from a disposable database already migrated to the exact packaged core**, into the release tree before `pack-update`:

```powershell
cargo run -p coa-release -- schema-contract --repack C:\games\release-fixture --tree C:\games\release-tree
```

The regular packager includes the contract and its hash in the signed manifest. Never capture it from a user's server or from the previous release database.

## Simultaneous worlds

On Windows the checkbox creates Wildcard when needed and saves distinct secondary world/RA ports. Both worlds start with one shared authserver/MySQL and separate world/character databases. The second world has its own executable copy, config, supervisor, logs and report spool in `.realms/secondary`; Data is read through the primary installation's absolute path. This runtime folder is excluded from packages.

The selected realm remains the primary world used by the Manager's console, companions, settings and Play. Selecting another realm still stops/restarts both to preserve that relationship. CoA companion restrictions on Wildcard remain in force. Both world statuses are shown on Overview.

Stop shuts down the secondary supervisor/world before the primary launcher may stop the shared database. Database restores, updates and profile changes reject a running secondary world. Each start refreshes its executable/config copies, so server updates cannot leave an old secondary binary behind. The secondary world inherits the primary network bind setting; its RA console remains on loopback. Enabling friends networking opens/maps both world ports. If networking was enabled before the second realm, enable it again to add the second port's rule/mapping.

## Disposable probes

```powershell
cargo run -p coa-core --example multiworld_e2e -- C:\games\your-fixture
cargo run -p coa-core --example repair_e2e -- C:\games\your-fixture
cargo run -p coa-core --example schema_e2e -- C:\games\your-fixture
```

The first checks both processes, ports, shared services and graceful shutdown. The second uses a test-only signing key, corrupts a fixture file, applies SQL to both worlds, verifies backups and account/character counts, and drops a column only in Wildcard to prove schema checks are isolated. These probes modify the fixture; never use a real server.
