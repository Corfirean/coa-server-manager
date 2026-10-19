---
area: manager
type: fixed
audience: admins
title: Large server updates no longer repeatedly rewrite the recovery journal
---
Updates containing rebuilt navigation data now prepare all recovery copies before replacing files. Interrupted updates remain recoverable without rewriting the full journal for every file.