---
area: manager
type: fixed
audience: admins
title: The database moves to a free port when Windows reserves or another program takes 3307
---
Hyper-V, WSL and Docker can reserve a range that includes the database port; the start then waited 30 seconds and failed. The database now picks a free port and records it, and a reserved game port is named as such at once.
