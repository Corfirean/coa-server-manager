# Realm profiles

**English** · [Русский](ru/WILDCARD.md) · [Deutsch](de/WILDCARD.md) · [Français](fr/WILDCARD.md) · [Español](es/WILDCARD.md) · [简体中文](zh-CN/WILDCARD.md)

Overview offers CoA and Wildcard. The installed worldserver must contain Wildcard support and the database must
already have its Wildcard migrations. The Manager refuses an unsupported binary, an occupied realm ID 2 or an
existing destination database rather than overwrite them.

CoA and Wildcard have separate character and world databases (`acore_characters` / `acore_world` for CoA, and
`acore_characters_wildcard` / `acore_world_wildcard` for Wildcard). Accounts remain in shared `acore_auth`.

When simultaneous realm execution is enabled in configuration (`simultaneous: true`), both CoA and Wildcard
realms can run concurrently:
- Primary and secondary world ports (`ports.world` and `secondary_world_port`, e.g. 8085 and 8086) and RA ports
  are distinct.
- Remote routing (both Direct Ingress and Relay links) is strictly bound to the specific ruleset realm selected:
  routing to a Wildcard realm targets the Wildcard world port and never advertises or connects to the CoA port.
- When simultaneous execution is disabled, only the currently active realm runs and provides world service.

Configuration profiles are saved under `Settings/realm-profiles`. An interrupted switch can restore its previous
configuration from the journal. CoA companion bots cannot be enabled on Wildcard. Content Scaling and the other
optional modules retain their settings but show experimental compatibility; class mechanics need separate gameplay
validation. Switching back restores the CoA module settings.

Quick/database backups include both character databases and shared auth. Full backups also include both worlds.
Recovery points identify their active realm; select that realm before restoring them. Server migrations apply to
both realms, with auth applied once. The server updater can replace its launcher: the Manager reapplies the realm
adapter before the next start.

## Portable Characters on Wildcard

As of v0.7.1, Wildcard realms participate in the Portable Characters subsystem with strict multi-ruleset isolation:
- **Canonical Model:** Wildcard portable characters carry `ruleset: "wildcard"` and `content_namespace: "wildcard"`.
- **Claim & Export:** Characters played on a Wildcard realm (`acore_characters_wildcard`) can be claimed and exported into canonical portable snapshots. Wildcard character builds—including spells, talents, skills, action bars, settings, inventory, pets, and currencies—are preserved.
- **Import & Session Runtime:** A Wildcard portable character can be transferred to a remote Wildcard host, imported into its `acore_characters_wildcard` database, armed in a runtime session (Session Protocol 3), and tracked through checkpoints and logout synchronization.
- **Collections:** Wildcard collections (`wildcard:appearance` and `wildcard:vanity`) are partitioned separately from CoA collections. Unlocks on a Wildcard realm union into the profile's Wildcard collections and never touch CoA collections.
- **Fail-Closed Protection:** Any attempt to transfer or import a CoA character into a Wildcard realm, or vice versa, is rejected at the protocol and database boundaries with zero mutation. If world-port resolution fails or a realm mode is inactive, Direct and Relay routing fail closed.

![Wildcard overview](screenshots/wildcard-demo/overview.jpg)

![Module compatibility](screenshots/wildcard-demo/modules.jpg)

[All guides](README.md)
