---
area: manager
type: changed
audience: admins
title: The Manager starts and stops servers without Python
---
Start, stop and forced stop run inside the Manager, and errors name their cause (a busy port, missing files, a startup failure). Servers that include Scripts/coa-launcher.exe also run the world supervisor and bug-report relay without Python.
