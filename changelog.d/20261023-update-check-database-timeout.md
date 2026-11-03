---
area: manager
type: fixed
audience: admins
title: A stalled database inspection no longer blocks server maintenance indefinitely
---
Update checks limit how long their database client can run and release the maintenance lock after a timeout. The Manager stops only its inspection client, keeps the database server running and reports that migration history could not be checked.
