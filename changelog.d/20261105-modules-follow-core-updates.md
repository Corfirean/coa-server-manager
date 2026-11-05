---
area: manager
type: added
audience: admins
title: Modules are rebuilt automatically after a core update
---
After the core is updated, the Manager rebuilds every enabled module for the new core before the first start, or reuses a build it already has. A module that cannot be rebuilt is switched off with the reason shown, and the core update still goes ahead. Rolling back the core puts the earlier module builds back.
