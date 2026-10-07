# Urgent update hotfix

Deadline: 2026-10-06. This checkout starts at the exact v0.6.4 tag, commit
6c771fa, and excludes the ongoing isolated-trial/recovery redesign.

The client accepts checksum compatibility only from a verified signed manifest and only
for an Applied migration. It never updates the historical checksum, reruns historical
SQL, or excuses a Running/Failed ledger row. A server package must explicitly list the
verified published variants and append corrections under new migration IDs.

Current local evidence: the locked workspace tests passed, including 252 core tests
run serially. Six frontend logic tests and the frontend production build passed.
The hotfix runtime was tested against the exact signed candidate manifest on the marked
CoA/Wildcard fixture. Both published repair checksums were accepted independently in
both worlds, with zero migration executions, unchanged historical checksums, and unchanged
account, character and inventory counts. The original ledger rows were restored after
the test, and all fixture processes were stopped. A real Running or Failed repair row in
either world was rejected before staging or creating a transaction, with unchanged world
executable and installation metadata. Tests use independent hotfix metadata so experimental
transaction formats are not confused with the 0.6.4 upgrade path. Windows journal writes also use flushed
atomic replacement with long-path support. Changelog validation passed.

Additional guards implemented:

- Read migration history without creating its ledger before downloading or replacing files.
- Reject missing journals, invalid identities, paths, hashes and inconsistent operations.
  Durable completed-cleanup markers distinguish interrupted pruning from a lost journal.
- Preserve all backups while an update is unfinished or its journal is damaged.
- Verify saved-file hashes and the complete database/configuration recovery point before
  stopping services or changing files/databases. New snapshots include configuration hashes;
  update journals bind their backup metadata to a hash.
- Surface state-reading errors in Settings and disable applying until the disk state is
  successfully read. Checking again refreshes that state.
- Verify every extracted SQL artifact before taking the update snapshot or replacing
  server files; missing files, invalid IDs, duplicate migrations and checksum disagreements
  reject the update early. SQL application repeats the integrity check immediately before use.
- Count unapplied migrations in the active and secondary realms, counting shared auth once.
  Matching versions and files do not hide remaining database work. Preview reads history
  under the installation lock; a stopped database is temporarily started for inspection
  and stopped again when no game services are running.
- Reread the disk state after every update/recovery attempt, including failed attempts and
  background updates. Unfinished or unreadable state blocks another application in the UI.
- Acquire the installation lock before checking and deleting a backup.

Latest real-runtime test used the independently copied, doubly marked fixture
`C:\games\coa-schema-fixture-chaos-20261005-v1` and signed candidate `0.261005.24`, manifest SHA-256
`1284b1273815209c9621b6d173a2211bae316f33462ccbe8b578e53b8d26fb5b`.
All 1748 SQL artifacts passed verification after authenticated archive extraction.
Removing one repair record separately in CoA and Wildcard increased the pending count
by exactly one; restoring its original record restored the original count. No migrations
were executed, player counts were preserved and fixture services were stopped afterward.
The source checkout remains uncommitted; these results are not an installer acceptance report.

Additional acceptance on 2026-10-06: a new independent installation/database copy at
`C:\games\coa-schema-fixture-chaos-064-transition-20261006` received the authenticated
published `0.261005.22` program files, original CRLF repair checksum, and a deliberately
missing Wildcard table in both realms. The production hotfix update pipeline committed
signed candidate `0.261005.24`, repaired both realms, passed schema checks and actual
bundled-Python server startup, and preserved the selected account, character, item and
inventory fields byte-for-byte. The fixture was stopped afterward. The harness explicitly
instruments the launcher to prevent relay submissions and bind to localhost; this is not
proof of packaged UI behavior or operating-system network containment.

Local installer `C:\games\source\manager-065-local-installer-20261006\CoA Server Manager_0.6.5_x64-setup.exe`
has SHA-256 `520ee0391d923b92da0a20591e30006c048cdb60156275d7a829918bf1b77491`.
Its updater signature and the published 0.6.4 installer's signature passed independent
verification with the configured updater public key. Both executables extracted from
their exact NSIS packages started, created responsive windows, and were stopped using
an independent application/WebView profile. No installer was executed over a user's
installation, and no UI update operation was exercised.

Legacy limitation: old 0.6.4 transaction copies and configuration snapshots without recorded
hashes can be checked for presence and declared database hashes, but their original file
contents cannot be authenticated retroactively. Missing copies block restoration. This
hotfix does not claim that old unchecksummed copies received full integrity verification.

Remaining release requirements: reproduce the existing-user failures through the published
0.6.4 interface, verify legacy unfinished transaction handling on real-runtime fixtures,
and execute the exact installer over 0.6.4 in an isolated Windows environment with UI
state/settings/server-list verification. Windows native UI automation is unavailable in
this session. Packaged startup and signature checks do not satisfy that installer gate.
No public release has been made.

The user subsequently exercised the packaged UI: candidate 0.261005.24 committed and
both CoA and Wildcard were shown running. Repeat-check/restart acceptance is still pending.
The screenshots exposed an older published package being offered after the local candidate
was installed. Follow-up changes reject older packages before preview/database inspection
and application and clear a cached offer after a failed recheck. All 253 core tests and
seven frontend tests passed. The earlier installer hash above does not include this follow-up;
a separately recorded revision is required. Native UI control was subsequently initialized
through the deferred node_repl tool, then the user stopped automation with Escape and
performed the UI check manually.

Another user repeat-check exposed the already installed launcher being offered again.
The preview now verifies canonical launcher bytes against the current signed package,
caches those bytes by SHA-256, and compares only recognized Manager transformations.
The actual disposable fixture's repeat preview passed with all 104 files skipped and
zero pending SQL. User edits, recorded custom content, and a package with different
launcher bytes are not mistaken for that integration. All 254 core tests passed;
the strengthened regression test and seven frontend tests also passed. The first
preview of a managed launcher may download/extract its authenticated package to build
the optional canonical cache. Earlier installer revisions do not include this fix.

Do not recommend deleting an installation or replacing its player databases.
