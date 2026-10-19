---
area: manager
type: fixed
audience: admins
title: Failed repairs restore the previous server after final verification errors
---
Errors after file replacement now undo the repair using its verified recovery point, including final file or database check errors. If recovery fails, the journal retains the cause and further server actions remain blocked.
