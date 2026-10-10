---
area: manager
type: fixed
audience: admins
title: A failing database update is no longer reported as "database not running"
---
An SQL error whose message contained the number 2003, such as an out-of-range value, was shown as "The database is not running". The real error is now shown.
