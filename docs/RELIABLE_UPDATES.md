# Reliable updates and Windows acceptance

An operation resolves one signed manifest and immutable release before downloading its
archive. The same verified bytes are used for the private trial and the real application.
The installation lock covers stop, snapshot, trial, application, validation and recovery.
Native Windows installations with the bundled Python and MySQL runtime are supported;
Docker and unrecognised launchers are rejected before applying files.

The manager measures the installation and database directories, checks free space and
allocates recovery reserves. The private copy contains independent runtime, configuration,
game-data and database files. It imports the verified original recovery point, runs the
candidate SQL and uses loopback ports for startup. The known launcher reporting relay is
disabled during validation. No junctions or hardlinks are created.

Transactions keep the exact manifest hash, original running state, protected generated
files, database recovery point and individual file operations. SQL has its own migration
ledger. Stop, snapshot and trial have separate durable states. The original failure stage
is retained when the current state changes to recovery or recovered.
Interrupted SQL is restored, never automatically repeated. Windows file replacement
uses a synced temporary file and MoveFileExW with WRITE_THROUGH, including long paths.
Recovery is repeatable and does not require backing up the damaged state. Corrupt journals
or snapshots block startup and preserve available copies.

Recovery verifies the original dump before importing it into a private recovery schema,
then compares the live database with a deterministic dump of that schema. This accounts
for MySQL's normalisation of character-set declarations while checking schema and rows.
Configuration backups carry individual content hashes. Legacy update snapshots without
those hashes require manual inspection rather than an unverified automatic restore.
Database dumps and configuration copies are flushed to disk before the snapshot is usable.
A committed operation interrupted before admission
resumes startup without undoing the committed database. Diagnostic JSON is saved locally;
database passwords are removed and reports leave the machine only through explicit export.

## Migration publication

`coa-release archive-migrations --package DIR --registry DIR` verifies the released
signature and preserves original SQL bytes, fixed order, hashes and signed provenance.
Only byte variants differing in UTF-8 BOM or line endings can share an ID. Other changes
fail the build and need a new migration ID. Compatibility hashes are included in the signed
manifest, leaving existing ledger checksums unchanged.

Published packages use `server-VERSION` releases. Channels contain one signed envelope;
it binds the immutable manifest and acceptance report hashes. Stable is switched only
after package verification and acceptance. Never replace assets of an existing immutable
release or rewrite historical registry SQL.

## Disposable acceptance fixture

Run `tools/update-chaos-gate.ps1 -BasePackage DIR -CandidatePackage DIR -Target NEW_PATH`
on Windows. NEW_PATH must start with `coa-schema-fixture-chaos-`; the harness creates
both disposable markers itself. Base and candidate signatures are required. The harness
extracts game data independently and seeds an account, character and inventory item.
Optional `-DataSource DIR` reads an existing game-data folder without modifying it.

Crash injection is compiled only with `coa-core/testkit` and additionally requires the
disposable marker. `COA_UPDATE_CRASH_AT` terminates the test process with exit 91 at a
named checkpoint. Production builds do not enable this mechanism. The harness exercises
file journaling, SQL phases, commit and repeated recovery interruption. Backups and failed
trial copies remain available for diagnosis.

The release jobs must run on a Windows worker with enough free space for the actual
signed base, private copy, full snapshots and recovery reserves. Insufficient space is a
test failure, not permission to bypass acceptance.
The workflows require a dedicated runner labelled `coa-update-acceptance` (Windows,
X64). Provision at least 300 GB of free workspace and 16 GB of RAM for the retained
ten-scenario matrix, plus the normal Rust/Tauri build prerequisites. The timeout is
24 hours: full snapshot, import and recovery checks take minutes per injected fault.
A default hosted runner's disk and four-hour budget are insufficient for this gate.

`tools/update-chaos-matrix.ps1` runs separate fixtures for CoA, Wildcard, both worlds,
SQUID and imported installations. With `-PreviousPackage`, it also runs each mode from
that signed previous release. Its acceptance report binds every scenario to the same
candidate manifest hash and manager source commit. Publication requires all ten scenarios.
It also requires every mandatory fault class in each scenario's coverage record. Missing
coverage produces an `incomplete` report and fails publication. The current harness still
lacks real disk exhaustion, denied writes, database-child and new-server crash coverage,
OS network containment and coverage of every file-operation phase.
The gate includes real occupied ports and locked files, a Python startup failure, and SQL
that creates and populates a table before failing. These faults must preserve seeded
accounts, characters and inventory in every applicable database.

Validation currently isolates listeners on loopback and disables the recognised reporting
relay. It does not enforce an operating-system firewall against arbitrary outbound actions
in custom binaries; additional network containment is required for that release criterion.

## Release acceptance still required

Unit tests and compilation do not establish the full release criterion. Record results
for exact package and installer hashes, previous supported releases and skipped versions,
CoA/Wildcard/both/SQUID, imported repacks and modified configurations. Exercise occupied
ports/files, partial DDL, disk exhaustion, write errors, corrupt packages/journals/backups,
Python failure and server crashes. Verify schema, all player data, migration history,
metadata and startup after every injected failure. A passing subset must not be described
as completion of this matrix or as a guarantee against all possible failures.
