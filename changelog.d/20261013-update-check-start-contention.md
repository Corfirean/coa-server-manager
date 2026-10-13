---
area: manager
type: fixed
audience: admins
title: Starting the server waits for update checks and identifies ongoing maintenance
---
Downloading an update manifest no longer blocks startup. A start waits up to 30 seconds for a check to finish; other active maintenance still blocks it safely and shows its operation, process and start time.
