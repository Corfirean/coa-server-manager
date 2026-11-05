---
area: manager
type: fixed
audience: admins
title: Update validation keeps MySQL running between compatible checks
---
Database checks and the temporary server startup share one MySQL session. The server still stops before files or database snapshots are replaced or restored.
