---
area: manager
type: fixed
audience: admins
title: Updates repair missing database defaults from the verified release schema
---
Only missing literal defaults are restored when column properties match. Existing rows, custom defaults, types and nullability are preserved; unresolved incompatibilities still block the update.
