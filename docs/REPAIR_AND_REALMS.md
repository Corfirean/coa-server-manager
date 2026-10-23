# Updates, backups and repair

**English** · [Русский](ru/REPAIR_AND_REALMS.md) · [Deutsch](de/REPAIR_AND_REALMS.md) · [Français](fr/REPAIR_AND_REALMS.md) · [Español](es/REPAIR_AND_REALMS.md) · [简体中文](zh-CN/REPAIR_AND_REALMS.md)

## Update your server

Select your server in the Manager. When an update is available, choose **UPDATE** and follow the prompts. Keep the Manager open until it finishes: it stops the server, creates a recovery point and checks the updated server.

Updates preserve accounts, characters and settings and cover every realm you have created. If an update fails, read the displayed error before trying again; recovery information remains available in the Manager.

## Keep a backup

Use **Backups** to save characters, accounts and settings before important changes. Full backups also include world data. Check the realm recorded in a recovery point before restoring it, and stop the server before a restore.

Portable character saved versions do not replace server backups. **Portable characters are experimental.**

## Check or repair an installation

1. Stop the server.
2. Open **Settings** and run **Check files and database**.
3. Review the results. If official server files need restoring, use **Repair** and follow the prompts.

Repair creates a recovery backup and restores official files while preserving settings and player databases. It may require the installed version to match the currently published signed package. Missing player data requires a backup; some database problems require a corrective server update.

If an operation stays blocked or fails again, use **Report a problem** and include diagnostics and the error message. Do not delete database files or recovery records to clear an error.

## Run CoA and Wildcard together

In **Settings**, enable **Start CoA and Wildcard together**. Both realms share accounts but keep separate characters and world data. Both statuses appear on Overview, and Stop shuts down both.

If friends networking was configured before enabling the second realm, run its setup again so both realms are reachable. Disable simultaneous startup before switching to Warcraft Reborn.

[All guides](README.md)
