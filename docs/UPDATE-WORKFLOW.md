# Update workflow

Version compatibility selects a package; it does not prove that a particular installed database can apply it.

The updater verifies the signed manifest and archive, checks file changes, stops the server, and saves one complete recovery point. Rehearsal and installation call the same database migration executor. That executor checks both realms before changing either, applies shared authentication migrations once, and uses the same pending-SQL selection as the preview count. Applied migrations are skipped; changed checksums and interrupted/failed records stop the update. Duplicate SQL identifiers for the same schema are rejected before execution.

The existing recovery journal, configuration merge, private-copy checks and startup verification remain enabled. This change does not relax the package compatibility matrix, force a migration through its SQL guards, or mark unexecuted SQL as applied.

## Verify with an actual installation copy

`cargo run -p coa-core --example upgrade_fixture -- <copy-root> <signed-package-directory>` exercises the production updater, including recovery copies and startup. It refuses mutation unless the directory name starts with `coa-schema-fixture-real` and contains `.coa-update-fixture`. Prepare a disposable copy from a verified recovery point, with separate ports, metadata and no client path. Never point it at a working installation.

`migration_plan <copy-root> <signed-manifest>` reads database history without creating a migration ledger. On a separate marked `update-database-diagnosis-*` copy, `--apply <migration-directory>` exercises the production database phase. `--sql-only <migration-directory>` verifies and applies the signed SQL list for both realms and compares account/character identities. SQL-only success is not full package/startup qualification and does not verify the target schema contract.

Record package signatures, exact source recovery point, actual result, repeat-run behavior and unavailable stages. Do not label a route universally tested based on a version list or SQL-only success.
