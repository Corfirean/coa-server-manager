---
area: manager
type: fixed
audience: admins
title: Diagnostic files with database checks stay readable after secrets are removed
---
Removing a line that mentioned a secret could break the JSON files in a diagnostic package. Secret values are now replaced instead, so the files stay valid.
