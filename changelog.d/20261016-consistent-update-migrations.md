---
area: manager
type: changed
audience: admins
title: Server updates apply migrations once after saving a recovery point
---
Updates no longer build and start a second full server copy. The Manager checks both realms, saves recovery, applies pending SQL once and verifies startup. Failed SQL or startup restores the recovery point. Full-copy upgrade testing remains a release task.
