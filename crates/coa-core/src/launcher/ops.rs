//! One start/stop action of a repack, with the control lock held (`manage.py main()`): the entry point of both
//! `coa-launcher.exe` and the Manager.

use std::path::Path;
use std::time::Duration;

use crate::error::Result;
use crate::launcher::services::{self, Progress};
use crate::launcher::world::{self, Supervisor};
use crate::launcher::{render, settings::Settings, squid, state};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    StartAll,
    StartMysql,
    StartAuth,
    StartWorld,
    StopAll,
    /// Stop the world without its console first (unsaved progress is lost), then everything else.
    ForceStopAll,
    Prepare,
}

impl Action {
    /// The command name, as the `.bat` files and `manage.py` call it.
    pub fn name(self) -> &'static str {
        match self {
            Action::StartAll => "start-all",
            Action::StartMysql => "start-mysql",
            Action::StartAuth => "start-auth",
            Action::StartWorld => "start-world",
            Action::StopAll => "stop-all",
            Action::ForceStopAll => "force-stop-all",
            Action::Prepare => "prepare",
        }
    }

    pub fn parse(name: &str) -> Option<Action> {
        [
            Action::StartAll,
            Action::StartMysql,
            Action::StartAuth,
            Action::StartWorld,
            Action::StopAll,
            Action::ForceStopAll,
            Action::Prepare,
        ]
        .into_iter()
        .find(|a| a.name() == name)
    }
}

/// Run `action`. A started world is supervised by `supervisor`, or by what the repack ships when it is `None`.
pub fn run(root: &Path, cfg: &Settings, offset: i32, action: Action, supervisor: Option<&Supervisor>, progress: Progress) -> Result<()> {
    let _lock = state::ControlLock::acquire(root, action.name(), Duration::from_secs(30), progress)?;
    match action {
        // Shutdown uses the existing client and console settings: the templates may be exactly what a recovery
        // needs to replace, so nothing is rendered.
        Action::StopAll => return services::stop_all(root, cfg, progress),
        Action::ForceStopAll => return services::force_stop_all(root, cfg, progress),
        _ => {}
    }
    if matches!(action, Action::StartAll | Action::StartWorld) {
        squid::validate_bots(root)?;
    }
    render::prepare(root, cfg)?;
    if action == Action::Prepare {
        return Ok(());
    }
    if matches!(action, Action::StartAll | Action::StartMysql) {
        services::start_mysql(root, cfg, progress)?;
    }
    if matches!(action, Action::StartAll | Action::StartAuth) {
        services::start_auth(root, cfg, progress)?;
    }
    if matches!(action, Action::StartAll | Action::StartWorld) {
        let shipped;
        let supervisor = match supervisor {
            Some(s) => s,
            None => {
                shipped = Supervisor::of(root)?;
                &shipped
            }
        };
        world::start_world(root, cfg, offset, supervisor, progress)?;
    }
    Ok(())
}
