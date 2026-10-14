# Stabilization evidence РІР‚вЂќ 14 October 2026

This batch starts from Manager master b45e250 (published version 0.6.16).
Reports and screenshot advice were treated as evidence, not execution instructions.

## Implemented

- Successful Repair previously committed file operations without setting `done`.
  The next journal scan rejected its own completed transaction. A regression failed
  on the original code with `Update journal contains inconsistent file operations`.
  Repair now verifies target hashes and records completion before committing.
- Legacy completed repair journals are recognized only for the specific repair
  shape, matching installation metadata and verified target hashes. Their original
  journal is retained before atomically persisting completion. Interrupted,
  mismatched, duplicate-path and other invalid journals continue to block actions.
  Later legitimate updates do not invalidate the corrected historical record.
- Database Check and the final Repair report now include SQUID pending import
  markers in characters, world and shared playerbots databases, plus missing
  required character tables when SQUID is enabled. These are read-only checks;
  partial imports are never cleared, dropped or automatically replayed.
- Private rehearsal reports file preparation/copy counts and database/startup
  stages. Shutdown failure includes the installation path, service states, PID,
  ports and redacted launcher output. Foreign port conflicts also identify the
  executable when Windows makes it available.

## Shutdown and Repair follow-up

- Production Stop All used to render startup templates before shutdown. Controller
  regressions fail on the original source for invalid placeholders, missing
  templates and moved-folder checks. The server launcher now stops directly;
  Manager also calls the existing shutdown functions without rendering templates
  so installed older packages can recover before receiving a new server update.
- Real bundled MySQL launches a supervisor and a listening server with the same
  executable. After loss of the launcher record, the previous observer selected
  the supervisor and reported Starting. Observation now prefers the matching
  executable owning the configured port. Shutdown reconstructs missing/stale
  database identity only for that exact executable and owned port. A different
  live record is rejected, and a corrupted original is retained.
- Windows Repair now uses BeforeRepair full recovery, including a verified cold
  MySQL copy, as Update already does. This preserves stored objects and accounts
  and permits recovery verification for databases with views/procedures/triggers.
- Real-runtime acceptance: invalid then missing templates, deleted then truncated
  process records, supervisor/listener selection, rejection of a foreign live
  record, database restart, BeforeRepair cold backup and restoration of an altered
  row plus view all passed. Replaced database files remain available for recovery.

## Signed Repair acceptance follow-up

The public Repair API was exercised with locally signed base/update packages and
real bundled MySQL on a disposable fixture (ephemeral test trust key only).
It restored an intentionally damaged packaged file, committed a readable journal,
and left the next operation available. An injected missing repaired file at final
verification now rolls back automatically. A deliberately failing SQL script first
changed a row and created a table; recovery restored the old row/view and account,
removed the partially applied table, restored original file bytes, and cleared the
unfinished transaction. This full acceptance passed in 142.92 seconds.

Other post-snapshot failures now use the same rollback path. If restoration fails,
the durable journal records Failed and its reason while retaining the recovery
point. Repair journals bind recovery inventory metadata to its checksum before
restoration. Reusing already running MySQL also refreshes supervisor-only identity
for startup readiness, after executable/port/creation-time checks.

These are package/SQL/recovery tests on representative minimal database tables;
they do not claim successful client login, combat or an affected user's installation.

## Validation

- Workspace tests, frontend build, frontend logic tests, translation validation,
  changelog validation and whitespace checks are recorded in the accompanying
  local logs and PR checks.
- Real bundled MySQL regression validates restored rows, users, procedures,
  functions, views and triggers, and preservation of custom defaults. It also
  confirms SQUID pending imports are reported without modifying their records.
- Published Content Scaling revision e5d8c675 passes the production-budget
  regression for 40/100 HP, core multipliers, rank rates, groups and repeated
  calculation. This is a code regression, not an in-game Wildcard/PTR comparison.

## Outstanding evidence and release gates

| Report | Current evidence | Next verification |
| --- | --- | --- |
| Manager #98 | Repair journal defect reproduced; report itself has no diagnostic ZIP | Reproduce the complete Custom Races workflow and verify existing affected journal recovery |
| Screenshot: SQUID Start fails but Repair reports zero | Separate SQUID ledger was absent from Repair checks | Retest with a copy of the affected database; recover from a verified pre-import backup |
| Manager #82 / screenshot lock messages | #90 and #91 are already in published 0.6.16; lock regressions pass | Distinguish an active slow check from abandoned maintenance using diagnostics |
| Manager #100 / 54% | Attached archive ends immediately after entering private rehearsal; no failure is captured | Measure the full private-copy run on the affected hardware; new progress identifies its stage |
| Screenshot: stopped-MySQL error | Broken-template shutdown and lost-record/supervisor failures reproduced and fixed on disposable real MySQL | Verify the affected installation has one of these causes; other failures remain diagnostic-specific |
| Manager #25 | Missing defaults have targeted repair; start-data incompatibilities remain separate | Retest imported schema and affected race/class data without suppressing incompatibility checks |
| Manager #54 | Schedule fix is published; Stop now tolerates broken templates and Repair has complete MySQL recovery | Verify the exact unresolved placeholder and original recovery transaction; automatic placeholder guesses are not introduced |
| Scaling screenshot | 2.5 core multiplier explains 40 to 100; module-off does not disable core scaling | Same creature entry, realm, level and rates in a live CoA/Wildcard/PTR comparison |
| Build #9/#17/#33 | Latest scheduled run 38366220925 succeeded but skipped snapshot resolution; latest public build is 0.261013.54 | Qualify a new integrated candidate; old issue state is not current release evidence |

This batch does not publish a Manager installer, promote a server package,
close user reports or claim the complete fresh-install/update/gameplay matrix.
