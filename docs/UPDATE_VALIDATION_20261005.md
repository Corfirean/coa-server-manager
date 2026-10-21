# Local update validation, 2026-10-05

These results are a partial validation record, not a release acceptance report.
Tests use the independently copied, doubly marked disposable fixture
`C:\games\coa-schema-fixture-chaos-20261005-v1`. User installations are not modified.

Candidate: `0.261005.24`, signed manifest SHA-256
`1284b1273815209c9621b6d173a2211bae316f33462ccbe8b578e53b8d26fb5b`.
Published migration provenance was verified for `0.261005.0` and `0.261005.22`.
Their disputed repair differs only by line endings; both published hashes are retained.

Completed real-runtime checks:

- Base-to-candidate update, isolated trial, SQL, startup and commit.
- Repeat candidate application without replaying applied migrations.
- CoA and Wildcard together, including both world processes and player databases.
- Hard process termination after stop and after corrective SQL.
- Repeated recovery interruption after database restoration, followed by successful recovery.
- Occupied world port and exclusively locked configuration-default file, rejected before stop.
- Partial DDL and inserted data followed by invalid SQL in the live corrective migration.
  Automatic rollback completed and removed the injected table.
- Bundled Python failure during live startup after a successful private two-world trial.
  Automatic recovery passed exact pre-operation file and installation-metadata hashes,
  restored all six databases, and preserved seeded accounts, characters and inventory.
- Modified user template values survived recovery. The fixture changes the template,
  not the launcher-generated active file; this corrected an initial harness assertion.
- A modified manifest with its original signature was rejected before application.
- Hard termination immediately after recording a complete snapshot, followed by separately
  corrupted transaction JSON and compressed character database archive. Both recovery
  attempts refused without starting services or changing the world executable/installation
  metadata. After removing the injected corruption, rollback completed and preserved the
  seeded accounts, characters and inventory in both worlds.
- A live Python failure with both worlds originally running restored their ready state,
  all six databases, player data, configuration and installation metadata. An initial
  strict comparison exposed realm-profile regeneration after restart; the final protection
  pass was corrected and the real-runtime case passed on repetition. Test processes were
  explicitly stopped after verification.

Checks compare restored database content and preserve a seeded account, character and
inventory item in each applicable realm. Recovery also checks original file hashes and
installation metadata. The core unit suite passed 260 tests; frontend build and translation
validation passed. Changes after these runs require the relevant checks again.
The full 260-test run used `--test-threads=1`. A parallel run during native database
recovery encountered connection resets in two existing RA socket tests; those failures
are recorded rather than treated as update failures or hidden by the passing rerun.

Not yet established: the complete five-mode, base/previous-release Windows matrix;
published installer execution against fixtures; actual disk exhaustion and denied writes;
database-child termination during table swaps; every individual file mutation checkpoint;
operating-system containment of arbitrary external actions. Publication must stay blocked
until the required matrix and remaining release invariants have been demonstrated.

The matrix report now explicitly records fault coverage. Missing mandatory fault classes
produce an `incomplete` report and fail the gate, even when every scenario that was executed
passed. Disk exhaustion, denied writes, database-child and new-server crashes,
network containment and all file-operation phases still require actual evidence. Snapshot
corruption guards now have real-runtime coverage. The later backup-metadata hash and
backup-retention guards require their own runtime injection checks before release.

Infrastructure inspection found zero registered repository runners for both projects.
The dedicated Windows acceptance runner must be provisioned before these workflows can run.
