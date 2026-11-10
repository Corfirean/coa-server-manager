---
area: manager
type: added
audience: admins
title: Moving to a core with modules installs the modules you used
---
On a core that loads Content Scaling, Companions or SQUID's Playerbots as modules, the Manager installs them at the versions the core names before the first start. Bots follow what the server used: SQUID if it was on, else Companions. Removed modules stay removed.
