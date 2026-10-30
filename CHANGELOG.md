# CoA Server Manager changelog

Manager releases only. Updating the Manager does not update the server.

[Server changelog](https://github.com/Corfirean/coa-server-build/blob/main/CHANGELOG.md)

Published releases and previews. Pre-release entries do not change the stable update channel.

Dates follow GitHub publication (UTC); legacy records retain their archived dates. Newest releases first.

## 0.9.7 — 2026-10-30

- Show available update notes even when release history is cached.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.9.7) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.9.5...v0.9.7)

## 0.9.5 — 2026-10-30

- Account creation verifies a lost console reply against the requested login.
- Abandoned game relay tunnels close their local connection immediately.
- Keep disabled graphics components disabled during client updates.
- Force-stop an unresponsive world after a failed normal shutdown.
- Crash history includes reports from every world and Windows text files.
- Show the online bot count for Playerbots on the overview.
- Disable public server browsing in Settings.
- Keep deliberate stops and update shutdowns from triggering crash restart.
- Server startup waits for a responsive console before reporting success.
- Restore the existing Manager window when launching it again.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.9.5) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.9.4...v0.9.5)

## 0.9.4 — 2026-10-29

**Portable Characters are experimental and may contain bugs.**

- Settings group updates, startup and maintenance with translated release history.
- Update and launch without updating share a divided button.
- Maximizing the window keeps text and controls at a comfortable size.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.9.4) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.9.3...v0.9.4)

## 0.9.3 — 2026-10-29

**Portable Characters are experimental and may contain bugs.**

- Slow server startup no longer triggers an early update rollback.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.9.3) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.9.2...v0.9.3)

## 0.9.2 — 2026-10-28

**Portable Characters are experimental and may contain bugs.**

- New server installation waits for slower database startup.
- Find and edit CoA options in the existing server settings.
- Simultaneous update checks share results or wait their turn.
- Recovery can stop an unfinished world startup after restoring an older launcher.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.9.2) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.9.1...v0.9.2)

## 0.9.1 — 2026-10-28

**Portable Characters are experimental and may contain bugs.**

- Existing unsupported character records no longer block unrelated server updates.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.9.1) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.9.0...v0.9.1)

## 0.9.0 — 2026-10-28

**Portable Characters are experimental and may contain bugs.**

- Choose Parallel or Global level scaling in module settings.
- Report Content Scaling problems directly to its repository.
- Send custom race problem reports directly to their maintainer.
- Internet sharing restores the LAN address after server restarts.
- Configure custom race availability for new characters.
- Public listing offers every realm that starts together, not just the main one.
- Public listing is a short list of realms with a window for the details.
- Configure profession and defense skill gains in server settings.
- Starting a server sets the character classes of the realm chosen on Overview.
- Start Wildcard and Warcraft Reborn together with Conquest of Azeroth.
- Control portable characters and keep Manager running in the system tray.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.9.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.8.2...v0.9.0)

## 0.8.2 — 2026-10-27

**Portable Characters are experimental and may contain bugs.**

- Installs the verified CoA Custom 1.9.2 client patches and quest safety addon, while recognizing existing 1.9.1 installations.
- Server updates preserve the order of database migrations introduced during merges, preventing older snapshots from overwriting newer progression.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.8.2) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.8.1...v0.8.2)

## 0.8.1 — 2026-10-25

**Portable Characters are experimental and may contain bugs.**

- Install the matching CoA Custom 1.9.1 client files.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.8.1) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.8.0...v0.8.1)

## 0.8.0 — 2026-10-24

**Portable Characters are experimental and may contain bugs.**

- Server owners can allow portable characters per realm.
- Allowing portable characters also prepares the server for portable sessions.
- Managers stay compatible with the public connection service.
- Relay connections recover and use the ports of the installed server.
- Portable progress resumes after reconnecting or restarting the Manager.
- Older copies on another host may require maintenance before returning.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.8.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.7.3...v0.8.0)

## 0.7.3 — 2026-10-23

**Portable Characters are experimental and may contain bugs.**

- Portable characters open again after using an experimental Manager.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.7.3) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.7.2...v0.7.3)

## 0.7.2 — 2026-10-23

- Advanced settings cover creature melee damage, spell damage and health by rank.
- Internet sharing accepts existing port forwards to the same server.
- The Auction House Bot card has a single switch.
- More character and server browser controls now use your selected language.
- Online player counts correctly separate random bots from players.
- Custom races work with shared addon folders on another drive.
- Configure the day and night cycle from the module settings.
- A stalled database inspection no longer blocks server maintenance indefinitely.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.7.2) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.7.1...v0.7.2)

## 0.7.1 — 2026-10-22

Close the game, then open Modules → Custom Races → Repair Client Patch to update existing patches, or Install Client Patch for a new installation. Updating the Manager does not automatically replace game files or update your server.

**Portable Characters are experimental and may contain bugs.**

- Updated Custom Races client patches to the complete 1.8.3 package.
- Race patch installation now includes the hide-armor and talent-reset addons.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.7.1) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.7.0...v0.7.1)

[Guides and screenshots](https://github.com/Corfirean/coa-server-manager/blob/master/docs/README.md)

## 0.7.0 — 2026-10-21

Portable transfers into Warcraft Reborn are not supported yet.

**Portable Characters are experimental and may contain bugs.**

- Keep portable characters and their saved versions in the Manager, and use them on compatible realms.
- Make an existing character portable from a connected realm.
- Browse public realms and see their connection details.
- Link an existing realm account or create one on hosts that allow it.
- Switch between CoA, Wildcard and Warcraft Reborn realm profiles on supported server builds.
- Adjust creature health for different locations and enemy types in Content Scaling.
- Switch Custom Races patches faster, with visible feedback while changes are applied.
- Handle Windows requests for administrator permission when launching the game.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.7.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.22...v0.7.0)

[Player guides and screenshots](https://github.com/Corfirean/coa-server-manager/tree/d22ff09ac6fd7628aac14d90ec23f547cd9b4bf1/docs)

## 0.6.22 — 2026-10-19

- Updated Custom Races client patches to 1.8.
- Included the updated race appearance and cursor addons.
- Existing client files are preserved when installing or removing race patches.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.22) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.21...v0.6.22)

## 0.6.21 — 2026-10-16

- Server updates apply migrations once after saving a recovery point.
- Older server versions update directly without installing intermediate packages.
- Update validation recognizes an already stopped server.
- Recorded core SQL stays applied after old history tables are cleaned up.
- Switching realms no longer repeats shared account database updates.
- Update offers distinguish version support from database validation.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.21) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.19...v0.6.21)

## 0.6.19 — 2026-10-16

- Update checks no longer download a server archive to compare its launcher.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.19) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.18...v0.6.19)

## 0.6.18 — 2026-10-16

- Older servers find a tested intermediate update instead of being blocked.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.18) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.17...v0.6.18)

## 0.6.17 — 2026-10-15

- Already running MySQL is recognized before startup readiness checks.
- New servers install the current stable build directly.
- Install the matching client patches for Grummle.
- Failed repairs restore the previous server after final verification errors.
- Failed server updates show an error on the Overview screen.
- Private update checks show preparation stages and stop failure details.
- Repair recovery copies preserve all bundled database objects and accounts.
- Server checks report interrupted SQUID database imports.
- Shutdown remains available when startup templates or database records are damaged.
- Successful server repairs no longer block later server actions.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.17) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.16...v0.6.17)

## 0.6.16 — 2026-10-13

- Linux Docker servers follow Settings, sharing and backups (experimental).
- Local hosts can connect while sharing a server online.
- Custom race client installation follows the current upstream release.
- Docker friend mode requests a restart when remote client support changes.
- Module cards show installed server revisions and the client race patch version.
- Server updates accept long Windows paths inside the private test server.
- Diagnostics record blocked startup requests and maintenance owner information.
- Client race patches can be repaired and removed while preserving original files.
- Checking an already installed server update no longer reports incompatibility.
- Failed server startup shows service errors even when the launcher reported ready.
- Starting the server waits for update checks and identifies ongoing maintenance.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.16) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.15...v0.6.16)

## 0.6.15 — 2026-10-11

- Large server updates no longer repeatedly rewrite the recovery journal.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.15) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.14...v0.6.15)

## 0.6.14 — 2026-10-11

- Fixed server update checks failing with long Windows paths.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.14) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.13...v0.6.14)

## 0.6.13 — 2026-10-11

- Failed server startup after an update automatically restores the previous version.
- Windows update recovery preserves database procedures triggers and views.
- Custom races bot setting synchronizes Playerbots excluded race lists.
- Blocked updates identify database objects that prevent automatic recovery.
- Updates can follow signed channels that select a complete server release.
- Server updates check startup privately before allowing players to connect.
- Updates repair missing database defaults from the verified release schema.
- Server updates no longer replay module installation scripts on existing databases.
- Windows server updates are tested on a private copy before changing your installation.
- Server updates can require a tested combination of platform and installed version.
- Failed database checks keep a complete report after rollback.
- Players can install custom race patches when connecting to a friend.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.13) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.12...v0.6.13)

## 0.6.12 — 2026-10-10

- Background world servers no longer stop because the local console is closed.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.12) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.11...v0.6.12)

## 0.6.11 — 2026-10-10

- Server and Bots settings open without a blank window.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.11) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.10...v0.6.11)

## 0.6.10 — 2026-10-10

- Custom race client patches update to the latest races and appearances.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.10) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.9...v0.6.10)

## 0.6.9 — 2026-10-10

- Custom races can be enabled and their client patches installed from Modules.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.9) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.8...v0.6.9)

## 0.6.8 — 2026-10-10

- Linux: install from the install screen, play with Wine or Proton (experimental).
- A failing database update is no longer reported as "database not running.
- Unsaved settings are protected and crashed servers can restart automatically.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.8) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.7...v0.6.8)

## 0.6.7 — 2026-10-08

- Choose where backups are stored, for example on another drive.
- Bot settings with a fixed list of values show a drop-down.
- The bot dashboard loads completely and can be opened in the browser.
- Accounts of the SQUID bots are hidden from the Accounts list.
- A Dashboard tab shows what the SQUID bots are doing.
- A stopped server stays stopped after an update, unless you ask it to start.
- Checking for updates no longer fails with "Something went wrong" after a crash.
- Server updates no longer stop with "has a different checksum" on the Wildcard repair.
- A world server started from the coa-bots folder is recognised.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.7) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.6...v0.6.7)

## 0.6.6 — 2026-10-07

- Install a server from a package on Linux, running in Docker (experimental).
- The Auction House Bot card no longer asks for a character GUID.
- Choose and remember the local network address friends use to connect.
- The NPC Enchanter card explains how to place and remove the NPC.
- Diagnostic files with database checks stay readable after secrets are removed.
- One button collects all diagnostics, including crashes and Windows crash records.
- Problem reports go to the right project.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.6) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.5...v0.6.6)

## 0.6.5 — 2026-10-06

- Imported SquidBots servers warn before replacing their original binary.
- SquidBots settings follow the options shipped by the module.
- SquidBots shows its bundled release and includes it in problem reports.
- Server updates accept verified published migration variants without rerunning them.
- Server updates replace older launchers adapted by the Manager for Wildcard.
- Older server packages cannot replace a newer installation.
- Checking again no longer offers an already installed launcher update.
- Large SQUID database imports have more time to finish on first startup.
- Update history conflicts are detected before server files change.
- Pending database updates remain visible when the server version matches.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.5) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.4...v0.6.5)

## 0.6.4 — 2026-10-05

- Start your installed server or client without downloading an available update.
- Finish a server update that was blocked by a startup failure.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.4) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.3...v0.6.4)

## 0.6.3 — 2026-10-04

- Configure partial dungeon parties and open-world life steal in module settings.
- Download and update the game client without installing a server.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.3) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.2...v0.6.3)

## 0.6.2 — 2026-10-04

- Choose which item types the auction bot sells.
- Module settings now offer translated controls and an enemy damage slider.
- Failed server updates restore databases and files together.
- Configure SQUID Playerbots and prevent conflicting bot systems.
- Server packages must include a complete database schema check.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.2) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.1...v0.6.2)

## 0.6.1 — 2026-10-04

- Server multipliers now accept decimals typed with a dot or comma.
- Play launches only the game client without starting your local server.
- Check database structure and repair server files with a safety backup.
- Start CoA and Wildcard together from Settings.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.1) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.6.0...v0.6.1)

## 0.6.0 — 2026-10-03

- The game client follows the realm selected in the Manager.
- Configuration snapshots no longer overwrite each other.
- Experimental Linux startup and Docker server controls.
- Problem reports open in the default browser.
- Simplified Chinese is available in the Manager.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.6.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.5.0...v0.6.0)

## 0.5.0 — 2026-10-03

- Imported servers get the module config files they lack.
- Modules marked Soon stay greyed out.
- The Bots page is hidden when the module is not on the server.
- Updating the game client shows a progress bar on its button.
- Switch between CoA and Wildcard while keeping separate characters and module settings.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.5.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.4.0...v0.5.0)

## 0.4.0 — 2026-10-02

- Added module cards with switches and settings.
- Added searchable server settings and appearance and collection controls.
- Added account deletion with confirmation.
- Fixed update checks offering an already installed version.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.4.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.3.1...v0.4.0)

## 0.3.1 — 2026-10-01

- Added named realmlists and a switcher beside Play.
- Added account management and a prefilled GitHub problem-report form.
- Server updates continue when leaving the settings page.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.3.1) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.3.0...v0.3.1)

## 0.3.0 — 2026-10-01

- Download and update the game client through the Manager while preserving custom files.
- Server updates are checked automatically and offered on Overview.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.3.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.2.0...v0.3.0)

## 0.2.0 — 2026-10-01

- Added companion creation progress and controls to stop creation, take bots offline or remove them.
- Choose how many bots log in when the server starts.
- Added a control to log out a chosen number of bots and reduce server load.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.2.0) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.1.6...v0.2.0)

## 0.1.6 — 2026-09-30

- Overview shows world speed.
- Improved companion setup, including requests made while the server is stopped.
- Expanded translated settings and connection guides.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.1.6) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.1.4...v0.1.6)

## 0.1.4 — 2026-09-30

- Sharing settings are reapplied before server startup.
- Added a restart shortcut after enabling play with friends.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.1.4) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.1.3...v0.1.4)

## 0.1.3 — 2026-09-30

- The interface scales with the window.
- Each friend connection mode shows the realmlist line to use.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.1.3) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.1.2...v0.1.3)

## 0.1.2 — 2026-09-30

- Manager updates download in the background and install when the app closes.
- Missing module configuration files are created from the supplied defaults.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.1.2) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.1.1...v0.1.2)

## 0.1.1 — 2026-09-30

- Added translations throughout setup, server updates, backups, diagnostics and settings.
- Added clearer errors when server packages are unavailable or a server cannot be reached.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.1.1) · [Source changes](https://github.com/Corfirean/coa-server-manager/compare/v0.1.0...v0.1.1)

## 0.1.0 — 2026-09-30

- Initial Windows release: install or import a server and manage startup from one app.
- Configure server and bot settings, manage accounts, create backups and restore saved data.
- Launch the game client, set up sharing with friends and collect diagnostics.

[Release](https://github.com/Corfirean/coa-server-manager/releases/tag/v0.1.0)
