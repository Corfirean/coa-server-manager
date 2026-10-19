---
area: manager
type: changed
audience: admins
title: Older server versions update directly without installing intermediate packages
---
The Manager checks database history and applies missing SQL from the latest signed package. A version missing from the release test list no longer blocks the update. Unknown versions, interrupted migrations and downgrades remain blocked.
