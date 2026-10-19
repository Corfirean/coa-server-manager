# Update workflow

The Manager selects the latest signed package directly for any known older server version. The source-version list records release qualification; it is not an exclusive allowlist and does not prove that a particular installed database can apply the package. Unknown imported versions, downgrades, incompatible platforms and newer Manager requirements remain blocked.

The updater verifies the signed manifest and archive, checks file changes and database history, stops the server, and saves one complete recovery point. It then replaces files, applies pending SQL once, verifies the target schema and starts the updated server once for validation. Shared authentication migrations run once; both realm histories are checked before changing either. Account and character identities are compared before and after migration and startup.

Account migration history is stored in the shared account database. Read-only checks recognize legacy auth records from both realms. Write operations adopt existing records and preserve already-applied core history; unknown legacy checksums stay unknown. History is read in two batched client calls, with explicit physical schemas and consistent text collations. Use the current Manager for subsequent database operations.

Applied migrations are skipped; changed checksums and interrupted/failed records stop the update. Duplicate SQL identifiers for the same schema are rejected before execution. SQL or startup failure restores files and databases together. If recovery fails, startup remains blocked and the recovery artifacts remain available.

The per-update nested server rehearsal and intermediate-package routing have been removed. Full-copy upgrade/startup testing belongs to release qualification, on independent disposable installations. Cleanup of interrupted rehearsals from older Manager versions remains for compatibility. Signed packages, recovery verification, configuration merge and startup validation stay enabled. No migration guard is bypassed and no unexecuted SQL is marked applied. Retired source archives do not prevent a direct update, but the database history must still be consistent and target checks must pass.

## Verify with an actual installation copy

`cargo run -p coa-core --example upgrade_fixture -- <copy-root> <signed-package-directory>` exercises the production updater, including recovery copies and startup. It refuses mutation unless the directory name starts with `coa-schema-fixture-real` and contains `.coa-update-fixture`. Prepare a disposable copy from a verified recovery point, with separate ports, metadata and no client path. Never point it at a working installation.

`migration_plan <copy-root> <signed-manifest>` reads database history without creating a migration ledger. On a separate marked `update-database-diagnosis-*` copy, `--apply <migration-directory>` exercises the production database phase. `--sql-only <migration-directory>` verifies and applies the signed SQL list for both realms and compares account/character identities. SQL-only success is not full package/startup qualification and does not verify the target schema contract.

Record package signatures, exact source recovery point, actual result, repeat-run behavior and unavailable stages. Do not label a route universally tested based on a version list or SQL-only success.
