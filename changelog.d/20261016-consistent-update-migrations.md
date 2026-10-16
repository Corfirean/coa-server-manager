---
area: manager
type: fixed
audience: admins
title: Rehearsal and installation use the same database migration checks
---
Both phases use one migration executor and check both realms before changing either. Duplicate migration entries are rejected before SQL runs.
