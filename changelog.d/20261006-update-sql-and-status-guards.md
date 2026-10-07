---
area: manager
type: fixed
audience: admins
title: Pending database updates remain visible when the server version matches
---
SQL files are checked before replacing server files. Update and recovery attempts refresh their saved status, including errors. Backup deletion cannot race with an update.
