---
area: manager
type: fixed
audience: admins
title: Switching realms no longer repeats shared account database updates
---
Solo and Wildcard use one migration history for their shared account database. Previously applied records from either realm are recognized without running their SQL again.
