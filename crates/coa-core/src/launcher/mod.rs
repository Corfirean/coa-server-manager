//! The repack launcher in Rust: what `Scripts/manage.py` does, without Python.
//!
//! Formats on disk stay exactly those of `manage.py`, so the two can be used on the same server during the move
//! (an old `.bat` and a new Manager, or the other way round):
//!
//! * `.state/<service>.json`   process records `{pid, exe, created}` (see [`crate::process::ProcessIdentity`]);
//! * `.state/control.lock`     one start/stop action at a time: byte 0 locked, as `msvcrt.locking` does;
//! * `.state/control-owner.json`, `.state/configuration.json`, `.state/debug-logs.json`;
//! * the rendered configs (`Core/configs/*.conf`, `mysql/my.ini`, `mysql/admin-client.ini`) byte for byte.
//!
//! State files are written with a flush to disk before they replace the old ones (`manage.py` did not, and a
//! power cut could leave a zero-filled record that stopped every later start).

pub mod render;
pub mod services;
pub mod settings;
pub mod spawn;
pub mod squid;
pub mod state;
pub mod world;

pub use settings::Settings;
