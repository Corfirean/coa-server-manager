---
area: manager
type: added
audience: admins
title: Moving to a core with modules installs the modules you used
---
When a server moves to a core that loads Content Scaling, Companions or SQUID's Playerbots as modules, the Manager downloads them at the versions the core names, builds them and switches them on before the first start. Bots follow what the server used before: SQUID's Playerbots if it was switched on, otherwise Companions. A module you remove later is not installed again.
