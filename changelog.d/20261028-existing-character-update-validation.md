---
area: manager
type: fixed
audience: admins
title: Existing unsupported character records no longer block unrelated server updates
---
Updates compare each affected character with its saved pre-update state. Unchanged older problems are reported as warnings without changing characters. Newly missing race/class data still blocks the update and triggers recovery; schema and identity checks remain enforced.
