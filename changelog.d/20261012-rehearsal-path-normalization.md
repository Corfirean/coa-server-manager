---
area: manager
type: fixed
audience: admins
title: Normalize rehearsal server roots to prevent Windows path prefix errors
---
Server update rehearsals now canonicalize the installation root before collecting files, resolving a "prefix not found" error during rehearsal preparation on Windows.
