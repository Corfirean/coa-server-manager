# Realm profiles

Overview offers CoA and Wildcard. The installed worldserver must contain Wildcard support and the database must
already have its Wildcard migrations. The Manager refuses an unsupported binary, an occupied realm ID 2 or an
existing destination database rather than overwrite them.

The first switch copies the CoA world into `acore_world_wildcard` and creates `acore_characters_wildcard` from
the character schema and neutral server seed tables. Existing characters, card collections and progress are not
copied. Accounts remain in shared `acore_auth`. Realm IDs 1 and 2 share the world port; only the selected world
runs, and the inactive realm is listed offline. A switch while running stops the server gracefully and restarts it.

Configuration profiles are saved under `Settings/realm-profiles`. An interrupted switch can restore its previous
configuration from the journal. CoA companion bots cannot be enabled on Wildcard. Content Scaling and the other
optional modules retain their settings but show experimental compatibility; class mechanics need separate gameplay
validation. Switching back restores the CoA module settings.

Quick/database backups include both character databases and shared auth. Full backups also include both worlds.
Recovery points identify their active realm; select that realm before restoring them. Server migrations apply to
both realms, with auth applied once. The server updater can replace its launcher: the Manager reapplies the realm
adapter before the next start.

## Portable Characters on Wildcard

As of v0.7.1 (MR2), Wildcard realms fully participate in the Portable Characters subsystem with strict multi-ruleset isolation:
- **Canonical Model:** Wildcard portable characters carry `ruleset: "wildcard"` and `content_namespace: "wildcard"`.
- **Claim & Export:** Real characters played on a Wildcard realm (`acore_characters_wildcard`) can be claimed and exported into canonical portable snapshots. Wildcard character builds—including spells, talents, skills, action bars, settings, inventory, pets, and currencies—are fully captured.
- **Import & Session Runtime:** A Wildcard portable character can be transferred to a remote Wildcard host, imported into its `acore_characters_wildcard` database, armed in a runtime session (Session Protocol 3), and played live with automatic checkpoints and logout synchronization.
- **Collections:** Wildcard collections (`wildcard:appearance` and `wildcard:vanity`) are partitioned separately from CoA collections. Unlocks on a Wildcard realm union into the profile's Wildcard collections and never touch CoA collections.
- **Fail-Closed Protection:** Any attempt to transfer or import a CoA character into a Wildcard realm, or vice versa, is rejected at the protocol and database boundaries with zero mutation.

![Wildcard overview](screenshots/wildcard-demo/overview.jpg)

![Module compatibility](screenshots/wildcard-demo/modules.jpg)

[All guides](README.md)
