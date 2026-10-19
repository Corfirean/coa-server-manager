---
area: manager
type: fixed
audience: admins
title: Recorded core SQL stays applied after old history tables are cleaned up
---
Existing core migration records are preserved in Manager history. Unknown legacy checksums remain explicitly unknown, and interrupted or changed migrations are still rejected.
