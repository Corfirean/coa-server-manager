# Warcraft Reborn

Choose **Warcraft Reborn** on Overview to create or switch to this realm. The
installed worldserver must contain Warcraft Reborn support; an old build is
refused before any databases are created. Stop the client before switching to
have its saved realm updated immediately.

The profile uses `CoA.ClassModel = "wcr"`, game-mode mask `0`, live realm type,
and realm ID `3`. Accounts remain in the shared `acore_auth` database. World data
is initially copied from CoA into `acore_world_reborn`; the structure of
`acore_characters` is cloned into `acore_characters_reborn` without copying
players. Existing database names or realm ID 3 block creation.

Settings are stored in `Settings/realm-profiles/reborn.json`. Creating a profile
from Wildcard uses the saved CoA settings as its base, preventing inheritance of
Wildcard gameplay flags. Both bot systems are disabled and cannot be enabled
through this profile; their class mechanics have not been validated. Other
optional modules are marked experimental.

Backups, updates and database checks include every created realm. Restore keeps
using the realm recorded in the recovery point. The Windows launcher is upgraded
from the Manager's previous two-realm patch and routes Warcraft Reborn to its
own databases. Docker uses the same realm schema mapping.

Simultaneous startup continues to support CoA + Wildcard only. Disable that
option before switching to Warcraft Reborn; it cannot be enabled while Warcraft
Reborn is selected.

Automated checks cover profile configuration and restoration, old state files,
binary support detection, database routing, all-realm enumeration, client realm
selection, and upgrading/reversing the launcher patch. These checks do not
constitute a gameplay validation of Warcraft Reborn or optional modules.
