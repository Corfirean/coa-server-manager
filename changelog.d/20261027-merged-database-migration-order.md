---
area: manager
type: fixed
audience: admins
title: Server updates preserve the order of database migrations added during merges
---
Older database snapshots no longer run after newer updates when preparing server packages. This prevents outdated Manastorm reward ranges from overwriting newer progression data.
