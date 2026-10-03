# CoA Server Manager changelog

## 0.6.0 — 2026-10-03

- Added Simplified Chinese for the interface and all server and bot settings.
- The closed game client now follows the selected CoA or Wildcard realm. Play checks the saved realm before launching; existing settings are preserved and backed up.
- Fixed GitHub reports opening Windows Explorer instead of the default browser. Long reports use the clipboard fallback.
- Fixed configuration snapshots created in the same millisecond overwriting each other during restore.
- Added experimental Linux startup: persistent state uses the XDG data folder, Tailscale detection supports Linux, and a Wayland rendering workaround respects existing user settings.
- Added experimental start, stop and status controls for manually prepared Docker servers, with private database networking and a remote console bound to localhost.
- Linux extraction restores executable permissions for ELF binaries and scripts, and paths retain case-sensitive filesystem semantics.

### Availability

This release ships the Windows installer and signed updater assets. Linux support is a development preview, not a one-click installation flow. Docker installation/database setup, SQL operations and backups, updates, Wildcard realm profiles, and Wine/Proton client launching remain unfinished. Servers must be prepared manually and use Linux binaries built with the packaged configuration layout.

Server shutdown and starting-zone scaling issues require separate server releases and are not fixed by updating the Manager alone.
