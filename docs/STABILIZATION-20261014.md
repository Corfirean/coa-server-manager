# Stabilization evidence — 14 October 2026

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
| Screenshot: stopped-MySQL error | Generic error hid launcher/process details | Reproduce with new shutdown diagnostics; do not assume a timeout or kill another installation |
| Manager #25 | Missing defaults have targeted repair; start-data incompatibilities remain separate | Retest imported schema and affected race/class data without suppressing incompatibility checks |
| Manager #54 | Targeted schedule SQL fix is published; recovery placeholders are a separate report | Reproduce placeholder recovery from the exact transaction/configuration |
| Scaling screenshot | 2.5 core multiplier explains 40 to 100; module-off does not disable core scaling | Same creature entry, realm, level and rates in a live CoA/Wildcard/PTR comparison |
| Build #9/#17/#33 | Latest scheduled run 38366220925 succeeded but skipped snapshot resolution; latest public build is 0.261013.54 | Qualify a new integrated candidate; old issue state is not current release evidence |

This batch does not publish a Manager installer, promote a server package,
close user reports or claim the complete fresh-install/update/gameplay matrix.
