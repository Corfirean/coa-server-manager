---
area: manager
type: fixed
audience: admins
title: Server updates no longer replay module installation scripts on existing databases
---
Update packages contain incremental SQL only. Module table initialization and seed scripts remain available when preparing a clean installation, preventing existing content from being reset during updates.
