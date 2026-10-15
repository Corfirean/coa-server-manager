---
area: manager
type: fixed
audience: admins
title: Already running MySQL is recognized before startup readiness checks
---
A verified MySQL listener replaces a stale or supervisor-only launcher record before startup. Existing databases are reused instead of waiting for a port owned by a different process from the same executable.
