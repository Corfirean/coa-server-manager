---
area: manager
type: fixed
audience: admins
title: Shutdown remains available when startup templates or database records are damaged
---
Stop uses existing shutdown settings instead of rendering startup templates. Missing or stale MySQL records are recovered only from the exact installed executable and its owned port; foreign processes are never adopted.
