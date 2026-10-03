# AGENTS.md

<!-- changelog:start (managed by coa-server-build/changelog/changelog.py sync-instructions) -->
## Changelog (shared by all CoA repositories)

Every CoA project (server fork, bots, Content Scaling, Manager, renderer) feeds **one** changelog that is posted to the
community. When you finish a change that a **player** or a **server owner** would notice (a new feature, a changed
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
