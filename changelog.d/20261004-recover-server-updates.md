---
area: manager
type: fixed
audience: admins
title: Failed server updates restore databases and files together
---
Updates now save a full backup, detect interrupted SQL, and check required Wildcard tables. Unfinished updates block server startup until recovered. Update and repair recovery also restore the previous installation metadata.
