---
area: manager
type: fixed
audience: admins
title: Checking for updates no longer fails with "Something went wrong" after an interrupted start
---
A crash or power loss could leave a damaged status file that made every database start fail. The Manager now removes such files itself, still compares the files when the database is unavailable, and shows the real reason.
