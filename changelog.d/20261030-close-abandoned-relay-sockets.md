---
area: manager
type: fixed
audience: admins
title: Abandoned game relay tunnels close their local connection immediately
---
Dropping a tunnel shuts down its shared socket directly instead of relying only on a background message, releasing its local read and write connection.
