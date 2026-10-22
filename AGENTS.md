# AGENTS.md

<!-- changelog:start (managed by coa-server-build/changelog/changelog.py sync-instructions) -->
## Changelog (shared by all CoA repositories)

Every CoA project (server fork, bots, Content Scaling, Manager, renderer) uses the same fragment format. **Manager and server release histories are separate**; each repository publishes only its own shipped component changes. When you finish a change that a **player** or a **server owner** would notice (a new feature, a changed
behaviour, a fix for something people ran into, a removed feature, a known issue), add **one small file to
`changelog.d/` in the same commit**. Do not add one for internal work (refactors, tests, CI, documentation).

File name: `YYYYMMDD-short-slug.md` (today's date). Content:

```
---
area: bots          # server | bots | scaling | modules | manager | addon | client | renderer
type: fixed         # added | changed | fixed | removed | known-issue
audience: players   # players | admins   (admins = people who run a server or use the Manager)
title: Bots no longer take Realm First achievements
---
Optional: one short paragraph in plain English for the reader (at most 300 characters).
```

- English only. The title says what changed **for the reader**, not how (at most 90 characters, starts with a capital,
  no full stop). No code names, file names or commit language in the title.
- One file per change. Never edit or delete files that are already released; never write version numbers (the release
  step adds them).
- Check your file: `python changelog.py check --dir changelog.d` (download the script from
  https://raw.githubusercontent.com/Corfirean/coa-server-build/main/changelog/changelog.py ), or create it with
  `python changelog.py new --area bots --type fixed --audience players --title "..."`.
- Full guide with examples: https://github.com/Corfirean/coa-server-build/blob/main/changelog/GUIDE.md
<!-- changelog:end -->

## Shared build and release workflow

Every published Manager version, including a clearly marked Pre-release, belongs in this repository's
CHANGELOG.md and changelog/history.json. Its GitHub release notes use the same reviewed record.
Promotion updates that entry without duplicating it; Manager and server histories remain separate.

Before preparing or publishing a Manager/server release, read the maintained
[release runbook](https://github.com/Corfirean/coa-server-build/blob/main/docs/server-release.md)
and [build agent instructions](https://github.com/Corfirean/coa-server-build/blob/main/AGENTS.md).
Use the local build checkout when available; otherwise retrieve the current files from GitHub.

Recover the exact prepared run, source commit, version, hashes and artifact expiry before rebuilding.
Prepare-only is not publication permission. A normal owner command to release the identified candidate
includes its required stable promotion; do not stop at a server Pre-release or ask twice for the same action.
Reuse signed prepared Manager installers after verifying their source metadata, SHA-256, updater JSON
and signature against the matching source's updater public key. Do not rerun compilation just to upload
already verified assets, or overwrite a published version with different bytes.

Server promotion uses the persistent Windows PC and the shared workflow's verified caches. Preserve
production databases; run applicable local acceptance in disposable fixtures. Record the actual passed,
failed and unrun checks. Verify both GitHub Latest and the public signed stable pointer before reporting
that users can update. Leave the durable handoff described in the runbook for the next agent.
