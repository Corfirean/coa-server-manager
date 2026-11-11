//! Thin Tauri command layer. Every mutating command takes an installation *id* and resolves the path from the
//! registry; the frontend can never ask the backend to touch an arbitrary path (scan is read-only).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod browse_preferences;
mod docker_install;
mod window_preferences;

use coa_core::backup::{self, Kind, RecoveryPoint, Trigger, VerifyReport};
use coa_core::config::{self, Scope, SettingsView};
use coa_core::download::Cancel;
use coa_core::driver::{self, DriverOutcome, Verb};
use coa_core::error::UiError;
use coa_core::install::{self, Preflight, Source};
use coa_core::layout::{self, Classification, ScanReport};
use coa_core::platform::{self, Flavor};
use coa_core::portable::service::{self as portable_service, PortableRuntime, ServiceError};
use coa_core::process::{self, Observed};
use coa_core::ra::Ra;
use coa_core::registry::{metadata_dir_for, InstallKind, InstallMeta, MetaDir, Registry};
use coa_core::update::{self, Resolution};
use coa_core::{Error, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, Manager, State};

mod control_cmds;

/// Informational release notes only; never used to select or authorize an update.
#[tauri::command]
async fn release_history(component: String, target: Option<String>) -> std::result::Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        coa_core::release_history::load_for_target(&component, &data_dir().join("release-history"), target.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}

struct AppState {
    registry: Registry,
    /// Installation ids with a start/stop currently running (one action at a time per server).
    busy: Mutex<HashSet<String>>,
    /// Cancel handle of the installation currently running, if any.
    install_cancel: Mutex<Option<Cancel>>,
    /// Cancel handle of the game-client check or download currently running, if any.
    client_cancel: Mutex<Option<Cancel>>,
    /// The portable play runtime (the Host loop): started with the application, stopped with it.
    portable: Option<Arc<PortableRuntime>>,
    /// Publishing this Manager's realms to the central Registry (Phase 10): its own loop, independent of Player Mode.
    realm_registry: Option<Arc<coa_core::realm_registry::RegistryRuntime>>,
    /// The player's side of the control plane (Phase 12): identity, protected credentials, joining servers.
    control: Option<Arc<coa_core::control::join::PlayerControl>>,
    /// The host's side: one control link per published realm.
    host_control: Option<Arc<coa_core::control::host_manager::HostControl>>,
}

/// Starting and stopping the servers this Manager installed, for the portable runtime.
struct InstallControl {
    registry: Registry,
}

impl InstallControl {
    fn run(&self, install_id: &str, verb: Verb) -> Result<()> {
        let root = self
            .registry
            .list()?
            .into_iter()
            .find(|(i, _)| i == install_id)
            .map(|(_, p)| p)
            .ok_or_else(|| Error::UnknownInstallation(install_id.to_string()))?;
        let out = driver::run(&root, verb)?;
        if out.ok {
            Ok(())
        } else {
            Err(Error::Invalid(
                out.output
                    .lines()
                    .last()
                    .unwrap_or("The server did not answer.")
                    .to_string(),
            ))
        }
    }
}

impl portable_service::ServerControl for InstallControl {
    fn stop_all(&self, install_id: &str) -> Result<()> {
        self.run(install_id, Verb::StopAll)
    }
    fn start_database(&self, install_id: &str) -> Result<()> {
        self.run(install_id, Verb::StartMysql)
    }
    fn start_all(&self, install_id: &str) -> Result<()> {
        self.run(install_id, Verb::StartAll)
    }
}

fn runtime(state: &State<'_, AppState>) -> std::result::Result<Arc<PortableRuntime>, ServiceError> {
    state.portable.clone().ok_or_else(|| ServiceError {
        code: "stopped".into(),
        message: "The portable play service could not start; see the Manager's log.".into(),
        notes: vec![],
    })
}

/// Run one action of the portable play service on its own thread without blocking the interface.
async fn portable_call<R: Send + 'static>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&mut portable_service::PortableService) -> std::result::Result<R, ServiceError>
        + Send
        + 'static,
) -> std::result::Result<R, ServiceError> {
    let rt = runtime(state)?;
    tauri::async_runtime::spawn_blocking(move || rt.call(f))
        .await
        .map_err(|e| ServiceError {
            code: "other".into(),
            message: e.to_string(),
            notes: vec![],
        })?
        .and_then(|r| r)
}

#[tauri::command]
fn portable_state(state: State<'_, AppState>) -> portable_service::PortableState {
    state
        .portable
        .as_ref()
        .map(|r| r.state())
        .unwrap_or_default()
}

#[tauri::command]
async fn portable_preflight(
    state: State<'_, AppState>,
    character: String,
    realm: String,
) -> std::result::Result<portable_service::PreflightView, ServiceError> {
    portable_call(&state, move |s| s.preflight(&character, &realm)).await
}

#[tauri::command]
async fn portable_play(
    state: State<'_, AppState>,
    character: String,
    realm: String,
    account: Option<String>,
) -> std::result::Result<portable_service::PlayView, ServiceError> {
    portable_call(&state, move |s| {
        s.play(&character, &realm, account.as_deref())
    })
    .await
}

#[tauri::command]
async fn portable_local_characters(
    state: State<'_, AppState>,
    realm: String,
) -> std::result::Result<Vec<portable_service::LocalCharacterView>, ServiceError> {
    portable_call(&state, move |s| s.local_characters(&realm)).await
}

#[tauri::command]
async fn portable_make(
    state: State<'_, AppState>,
    realm: String,
    token: u32,
) -> std::result::Result<portable_service::CharacterView, ServiceError> {
    portable_call(&state, move |s| s.make_portable(&realm, token)).await
}

#[tauri::command]
async fn portable_resolve(
    state: State<'_, AppState>,
    character: String,
    realm: String,
    action: portable_service::Resolve,
) -> std::result::Result<(), ServiceError> {
    portable_call(&state, move |s| s.resolve(&character, &realm, action)).await
}

#[tauri::command]
async fn portable_history(
    state: State<'_, AppState>,
    character: String,
) -> std::result::Result<Vec<portable_service::HistoryEntry>, ServiceError> {
    portable_call(&state, move |s| s.history(&character)).await
}

#[tauri::command]
async fn portable_add_realm(
    state: State<'_, AppState>,
    path: String,
) -> std::result::Result<portable_service::RealmView, ServiceError> {
    portable_call(&state, move |s| {
        s.add_prepared_realm(std::path::Path::new(&path))
    })
    .await
}

#[tauri::command]
async fn portable_remove_realm(
    state: State<'_, AppState>,
    realm: String,
) -> std::result::Result<(), ServiceError> {
    portable_call(&state, move |s| s.remove_prepared_realm(&realm)).await
}

#[tauri::command]
async fn portable_diagnostics(
    state: State<'_, AppState>,
) -> std::result::Result<portable_service::DiagnosticsReport, ServiceError> {
    portable_call(&state, |s| Ok(s.diagnostics())).await
}

/// Start the game for a realm that is ready: an installed server uses its own client, any other realm the Player Mode client pointed at
/// the realm's address.
#[tauri::command]
async fn portable_launch(
    state: State<'_, AppState>,
    realm: String,
) -> std::result::Result<(), ServiceError> {
    let realm_launch = realm.clone();
    let (install, address) =
        portable_call(&state, move |s| s.realm_launch_info(&realm_launch)).await?;
    let other = |e: UiError| ServiceError {
        code: "launch".into(),
        message: e.technical,
        notes: vec![],
    };
    match install {
        Some(id) => play(state, id).await.map(|_| ()).map_err(other),
        None => {
            {
                let dir = remote_dir();
                let mut profile =
                    coa_core::remote_client::load(&dir).map_err(|e| other(e.into()))?;
                profile.host = address;
                coa_core::remote_client::save(&dir, &profile).map_err(|e| other(e.into()))?;
                if let Some(ref cp) = profile.client_path {
                    if let Ok(pc) = control_cmds::player(&state) {
                        if let Ok(rid) = control_cmds::realm_id(&realm) {
                            if let Ok(Some(creds)) = pc.credentials(&rid) {
                                let _ = coa_core::glue_config::prefill_login(
                                    std::path::Path::new(cp),
                                    &creds.username,
                                    &creds.password,
                                );
                            }
                        }
                    }
                }
            }
            play(state, REMOTE_CLIENT_ID.to_string())
                .await
                .map(|_| ())
                .map_err(other)
        }
    }
}

fn update_source(custom: Option<String>) -> Result<Source> {
    let pick = custom
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("COA_UPDATE_SOURCE").ok());
    match pick {
        Some(p) if p.to_ascii_lowercase().starts_with("http") => Ok(Source::Url(p)),
        Some(p) => Ok(Source::Dir(PathBuf::from(p))),
        None => coa_core::channels::preview_update_source(),
    }
}

const DEFAULT_PACKAGE_URL: &str =
    "https://github.com/Corfirean/coa-server-build/releases/download/base";

fn package_source(custom: Option<String>) -> Source {
    let pick = custom
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("COA_PACKAGE_SOURCE").ok());
    match pick {
        Some(p) if p.to_ascii_lowercase().starts_with("http") => Source::Url(p),
        Some(p) => Source::Dir(PathBuf::from(p)),
        None => Source::Url(DEFAULT_PACKAGE_URL.into()),
    }
}

fn installation_source(custom: Option<String>) -> Result<Source> {
    let custom = custom.filter(|value| !value.trim().is_empty()).or_else(|| {
        std::env::var("COA_PACKAGE_SOURCE")
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    // A new server is installed from the qualified stable image; the preview channel publishes updates only,
    // so the preview core arrives as the first update (the same path every existing server takes).
    select_installation_source(custom, coa_core::channels::stable_source)
}

fn select_installation_source(
    custom: Option<String>,
    latest: impl FnOnce() -> Result<Source>,
) -> Result<Source> {
    if custom.is_some() {
        return Ok(package_source(custom));
    }
    Ok(Source::Installation(Box::new(latest()?)))
}

#[cfg(test)]
mod installation_source_tests {
    use super::*;

    #[test]
    fn fresh_install_uses_the_selected_current_release_full_image() {
        let source = select_installation_source(None, || {
            Ok(Source::Url("https://example.test/releases/current".into()))
        }).unwrap();
        assert!(matches!(source, Source::Installation(inner)
            if matches!(*inner, Source::Url(ref url) if url.ends_with("/current"))));
    }

    #[test]
    fn unavailable_current_release_does_not_fall_back_to_an_old_base() {
        let result = select_installation_source(None, || {
            Err(Error::PackageNotPublished("current unavailable".into()))
        });
        assert!(matches!(result, Err(Error::PackageNotPublished(_))));
    }
}

#[derive(Serialize)]
struct ServerSummary {
    id: String,
    name: String,
    path: String,
}

#[derive(Serialize)]
struct StatusView {
    observed: Observed,
    busy: bool,
    path_exists: bool,
}

/// Where the Manager keeps its own state (server list, logs). `%LOCALAPPDATA%` on Windows, the XDG data folder elsewhere.
fn data_dir() -> PathBuf {
    // a separate data folder for tests and diagnostics, so that a development build never reads or writes the installed Manager's own state
    if let Some(dir) = std::env::var_os("COA_MANAGER_DATA_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return dir;
    }
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(windows))]
    let base = unix_data_home(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"));
    base.join("CSMModulesPreview")
}

#[allow(dead_code)]
fn stable_data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(windows))]
    let base = unix_data_home(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"));
    base.join("CoAServerManager")
}

/// `$XDG_DATA_HOME` when it is an absolute path (the spec says to ignore relative ones), else `~/.local/share`.
#[cfg(not(windows))]
fn unix_data_home(xdg: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> PathBuf {
    xdg.map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home.map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
}

fn path_of(state: &AppState, id: &str) -> Result<PathBuf> {
    state
        .registry
        .list()?
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, p)| p)
        .ok_or_else(|| Error::UnknownInstallation(id.to_string()))
}

fn summary(id: String, path: PathBuf) -> ServerSummary {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Server".into());
    ServerSummary {
        id,
        name,
        path: path.to_string_lossy().into_owned(),
    }
}

/// The game data folder of a Docker server; none for a repack, whose data is inside its own folder.
#[tauri::command]
fn game_data_folder(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Option<String>, UiError> {
    let root = path_of(&state, &id)?;
    if !coa_core::docker::is_docker(&root) {
        return Ok(None);
    }
    Ok(Some(
        coa_core::docker::game_data_dir(&root)?
            .to_string_lossy()
            .into_owned(),
    ))
}

/// Point a Docker server at another game data folder. The servers must be stopped.
#[tauri::command]
async fn set_game_data_folder(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> std::result::Result<String, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        if !coa_core::docker::is_docker(&root) {
            return Err(Error::Invalid(
                "The game data of this server is inside its folder.".into(),
            ));
        }
        coa_core::docker::set_game_data(&root, std::path::Path::new(&path))?;
        Ok(path)
    })
    .await
}

#[tauri::command]
fn default_install_dir() -> String {
    if platform::flavor() == Flavor::Docker {
        return docker_install::default_dir();
    }
    "C:\\Games\\CoA Server".into()
}

/// What the install screen needs to know about this computer: the kind of server it installs, the suggested folder, and
/// whether Docker can be used.
#[tauri::command]
async fn install_environment() -> docker_install::Environment {
    tauri::async_runtime::spawn_blocking(docker_install::environment)
        .await
        .unwrap_or_else(|_| docker_install::Environment {
            flavor: platform::flavor(),
            default_dir: default_install_dir(),
            docker_problem: None,
        })
}

#[tauri::command]
async fn scan_server(path: String) -> std::result::Result<ScanReport, UiError> {
    tauri::async_runtime::spawn_blocking(move || layout::scan(std::path::Path::new(&path)))
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
        .map_err(Into::into)
}

/// Import: writes only `<folder>.manager` and the registry entry. The server folder is never modified.
#[tauri::command]
async fn add_server(
    state: State<'_, AppState>,
    path: String,
) -> std::result::Result<ServerSummary, UiError> {
    let root = PathBuf::from(&path);
    let report = tauri::async_runtime::spawn_blocking({
        let root = root.clone();
        move || layout::scan(&root)
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))??;

    if report.classification == Classification::Incompatible {
        return Err(Error::Invalid("This folder is not a CoA server folder.".into()).into());
    }
    if let Some(existing) = state.registry.find_by_path(&root)? {
        return Ok(summary(existing, PathBuf::from(report.path)));
    }

    let mut meta = InstallMeta::new(InstallKind::Imported, std::path::Path::new(&report.path));
    meta.core.commit = report
        .release
        .as_ref()
        .and_then(|r| r.main_revision.clone());
    meta.core.version = report.banner_revision.clone();
    meta.database.port = Some(report.ports.mysql);
    meta.database.schemas = report.database_schemas.clone();
    // The executables are `worldserver.exe` in a repack and `worldserver` in a Docker server; the file check reads these names.
    let exe_names = if coa_core::docker::is_docker(std::path::Path::new(&report.path)) {
        ("Core/worldserver", "Core/authserver")
    } else {
        ("Core/worldserver.exe", "Core/authserver.exe")
    };
    for (rel, exe) in [
        (exe_names.0, &report.worldserver),
        (exe_names.1, &report.authserver),
    ] {
        if let Some(e) = exe {
            meta.original_hashes.insert(rel.into(), e.sha256.clone());
        }
    }
    let server = PathBuf::from(&report.path);
    let dir = metadata_dir_for(&server)?;
    let (meta, _dir) = if dir.join("install.json").is_file() {
        MetaDir::open(&dir).map(|(d, m)| (m, d))?
    } else {
        let d = MetaDir::create(&server, &meta)?;
        (meta, d)
    };
    state.registry.register(&meta.id, &server)?;
    tracing::info!(id = %meta.id, path = %server.display(), "server imported (read-only)");
    Ok(summary(meta.id, server))
}

#[tauri::command]
fn list_servers(state: State<'_, AppState>) -> std::result::Result<Vec<ServerSummary>, UiError> {
    Ok(state
        .registry
        .list()?
        .into_iter()
        .map(|(id, p)| summary(id, p))
        .collect())
}

#[tauri::command]
fn forget_server(state: State<'_, AppState>, id: String) -> std::result::Result<(), UiError> {
    // Removes only the registry entry; no files are deleted.
    Ok(state.registry.unregister(&id)?)
}

#[tauri::command]
async fn server_status(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<StatusView, UiError> {
    let root = path_of(&state, &id)?;
    let busy = state.busy.lock().map(|b| b.contains(&id)).unwrap_or(false);
    let path_exists = root.is_dir();
    let observed = tauri::async_runtime::spawn_blocking(move || {
        let ports = layout::read_ports(&root);
        process::observe(&root, &ports)
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(StatusView {
        observed,
        busy,
        path_exists,
    })
}

async fn run_verb(
    state: &AppState,
    id: String,
    verb: Verb,
) -> std::result::Result<DriverOutcome, UiError> {
    let root = path_of(state, &id)?;
    if !state
        .busy
        .lock()
        .map_err(|_| Error::Invalid("state poisoned".into()))?
        .insert(id.clone())
    {
        return Ok(DriverOutcome {
            ok: false,
            exit_code: None,
            code: Some(coa_core::ErrorCode::OperationInProgress),
            human: Some(coa_core::ErrorCode::OperationInProgress.human()),
            output: String::new(),
        });
    }
    let result = tauri::async_runtime::spawn_blocking(move || driver::run(&root, verb)).await;
    if let Ok(mut b) = state.busy.lock() {
        b.remove(&id);
    }
    result
        .map_err(|e| Error::Invalid(e.to_string()))?
        .map_err(Into::into)
}

#[tauri::command]
async fn start_server(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<DriverOutcome, UiError> {
    let _guard = BusyGuard::acquire(&state, &id)?;
    // Servers installed before module configs were created automatically get them now (missing files only). Imported
    // servers only get the module files they lack (see below); nothing they have is changed.
    if let Ok(root) = path_of(&state, &id) {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let (_, meta) = install_meta(&root)?;
            if meta.kind == coa_core::registry::InstallKind::New {
                coa_core::config::materialize_module_configs(&root)?;
            } else {
                // Imported: only the module files that are missing (a module without its file logs every setting it reads).
                let _ = coa_core::config::create_missing_module_configs(&root);
            }
            let dir = meta_dir(&root)?;
            coa_core::friends::ensure_bind(&root, &dir)?;
            if meta.kind == coa_core::registry::InstallKind::New {
                coa_core::config::ensure_performance_defaults(&root, &dir)?;
            }
            Ok::<(), Error>(())
        })
        .await;
    }
    let root = path_of(&state, &id)?;
    let out = blocking(move || driver::run(&root, Verb::StartAll)).await?;
    if out.ok {
        if let Ok(root) = path_of(&state, &id) {
            // Companions requested while the server was stopped are created now (once; a failure is only logged).
            let r = root.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || -> Result<()> {
                if coa_core::realms::guard_module(&r, "companions").is_err() {
                    return Ok(());
                }
                let pending = meta_dir(&r)?.join("companions.pending.json");
                if let Ok(v) = coa_core::fsx::read_json::<serde_json::Value>(&pending) {
                    let _ = std::fs::remove_file(&pending);
                    let n = v["count"].as_u64().unwrap_or(0) as u32;
                    if n > 0 {
                        let made = coa_core::db::Db::from_repack(&r, coa_core::db::Account::Admin)
                            .and_then(|db| coa_core::companions::ensure_templates(&db));
                        if let Err(e) = made
                            .and_then(|_| Ra::connect(&r))
                            .and_then(|mut ra| ra.spawn_bots(n))
                        {
                            tracing::warn!("pending companions were not created: {e}");
                        }
                    }
                }
                Ok(())
            })
            .await;
            let _ = tauri::async_runtime::spawn_blocking(move || {
                let meta = meta_dir(&root)?;
                coa_core::friends::reapply(&root, &meta)?;
                // A world switched on after the server was opened to friends has no firewall rule yet. Rules are only
                // added when missing (one Windows confirmation); a refusal must not undo a start that worked.
                if platform::flavor() == Flavor::Repack
                    && coa_core::friends::load(&meta).mode != coa_core::friends::Mode::Local
                {
                    let extra = coa_core::realms::state(&root)?.extra_world_ports();
                    if let Err(e) = coa_core::firewall::ensure_rules_with_extra(
                        &layout::read_ports(&root),
                        &extra,
                    ) {
                        tracing::warn!("firewall rules for the running worlds were not added: {e}");
                    }
                }
                Ok::<(), Error>(())
            })
            .await;
        }
    }
    Ok(out)
}

#[tauri::command]
async fn force_stop_server(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<DriverOutcome, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || driver::force_stop(&root)).await
}

#[tauri::command]
async fn stop_server(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<DriverOutcome, UiError> {
    run_verb(&state, id, Verb::StopAll).await
}

#[derive(Serialize)]
struct PresetInfo {
    id: String,
    title: String,
    description: String,
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T> + Send + 'static,
) -> std::result::Result<T, UiError> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
        .map_err(Into::into)
}

fn meta_dir(root: &std::path::Path) -> Result<PathBuf> {
    let dir = metadata_dir_for(root)?;
    if dir.join("install.json").is_file() {
        Ok(dir)
    } else {
        let internal_meta = root.join(".manager");
        if internal_meta.join("install.json").is_file() {
            Ok(internal_meta)
        } else if internal_meta.is_dir() || root.join("worldserver.exe").is_file() {
            // Auto-heal minimal install.json for imported or standalone spike realms
            let meta = coa_core::registry::InstallMeta::new(coa_core::registry::InstallKind::Imported, root);
            let _ = std::fs::create_dir_all(&internal_meta);
            let _ = coa_core::fsx::atomic_write_json(&internal_meta.join("install.json"), &meta);
            Ok(internal_meta)
        } else {
            Err(Error::Invalid(
                "This server has not been added to the Manager yet.".into(),
            ))
        }
    }
}

#[tauri::command]
async fn get_settings(
    state: State<'_, AppState>,
    id: String,
    scope: Scope,
) -> std::result::Result<SettingsView, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || config::load(&root, scope)).await
}

#[tauri::command]
async fn server_logging_preset(
    state: State<'_, AppState>,
    id: String,
    preset: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        coa_core::server_logging::apply(&root, &meta, &preset)
    }).await
}

#[tauri::command]
async fn save_settings(
    state: State<'_, AppState>,
    id: String,
    scope: Scope,
    changes: BTreeMap<String, Value>,
) -> std::result::Result<config::SaveReport, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        config::save(&root, &meta, scope, &changes)
    })
    .await
}

#[tauri::command]
fn list_presets(scope: Scope) -> Vec<PresetInfo> {
    scope
        .presets()
        .iter()
        .map(|p| PresetInfo {
            id: p.id.clone(),
            title: p.title.clone(),
            description: p.description.clone(),
        })
        .collect()
}

#[tauri::command]
async fn preview_preset(
    state: State<'_, AppState>,
    id: String,
    scope: Scope,
    preset: String,
) -> std::result::Result<config::PresetPreview, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        if preset == "defaults" {
            config::preview_defaults(&root, scope)
        } else {
            config::preview_preset(&root, scope, &preset)
        }
    })
    .await
}

#[tauri::command]
fn list_config_snapshots(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<config::SnapshotInfo>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(config::list_snapshots(&meta_dir(&root)?))
}

#[tauri::command]
async fn restore_config_snapshot(
    state: State<'_, AppState>,
    id: String,
    snapshot: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || config::restore_snapshot(&meta_dir(&root)?, &snapshot)).await
}

/// Marks the installation busy for the duration of a long operation, and always clears it.
struct BusyGuard<'a> {
    state: &'a AppState,
    id: String,
}

impl<'a> BusyGuard<'a> {
    fn acquire(state: &'a AppState, id: &str) -> Result<Self> {
        let mut b = state
            .busy
            .lock()
            .map_err(|_| Error::Invalid("state poisoned".into()))?;
        if !b.insert(id.to_string()) {
            return Err(Error::Invalid(
                "Another action is still in progress for this server.".into(),
            ));
        }
        Ok(Self {
            state,
            id: id.to_string(),
        })
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut b) = self.state.busy.lock() {
            b.remove(&self.id);
        }
    }
}

#[tauri::command]
fn backup_location(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<backup::BackupLocation, UiError> {
    let root = path_of(&state, &id)?;
    Ok(backup::location(&meta_dir(&root)?))
}

#[tauri::command]
async fn set_backup_location(
    state: State<'_, AppState>,
    id: String,
    path: Option<String>,
) -> std::result::Result<backup::BackupLocation, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || backup::set_location(&root, &meta_dir(&root)?, path.as_deref())).await
}

#[tauri::command]
async fn dashboard_status(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::dashboard::Status, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || Ok(coa_core::dashboard::status(&root))).await
}

#[tauri::command]
async fn dashboard_install(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::dashboard::Status, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        coa_core::dashboard::install(&root, &|step| {
            let _ = app.emit("dashboard-progress", step);
        })
    })
    .await
}

#[tauri::command]
async fn dashboard_open(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    let status = blocking(move || Ok(coa_core::dashboard::status(&root))).await?;
    if !status.running {
        return Err(Error::Invalid("The dashboard is not running.".into()).into());
    }
    tauri_plugin_opener::open_url(&status.url, None::<&str>)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(())
}

#[tauri::command]
async fn dashboard_start(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::dashboard::Status, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::dashboard::start(&root)).await
}

#[tauri::command]
async fn dashboard_stop(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::dashboard::Status, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        coa_core::dashboard::stop(&root)?;
        Ok(coa_core::dashboard::status(&root))
    })
    .await
}

#[tauri::command]
fn list_backups(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<RecoveryPoint>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(backup::list(&meta_dir(&root)?))
}

#[tauri::command]
async fn create_backup(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    kind: Kind,
    label: Option<String>,
) -> std::result::Result<RecoveryPoint, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        backup::create(&root, &meta, kind, Trigger::Manual, label, &|step| {
            let _ = app.emit("backup-progress", step);
        })
    })
    .await
}

#[tauri::command]
async fn verify_backup(
    state: State<'_, AppState>,
    id: String,
    backup_id: String,
) -> std::result::Result<VerifyReport, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || backup::verify(&meta_dir(&root)?, &backup_id)).await
}

#[tauri::command]
fn delete_backup(
    state: State<'_, AppState>,
    id: String,
    backup_id: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    Ok(backup::delete(&meta_dir(&root)?, &backup_id)?)
}

#[tauri::command]
async fn restore_backup_configs(
    state: State<'_, AppState>,
    id: String,
    backup_id: String,
) -> std::result::Result<RecoveryPoint, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || backup::restore_configs(&root, &meta_dir(&root)?, &backup_id)).await
}

#[tauri::command]
async fn restore_backup_database(
    state: State<'_, AppState>,
    id: String,
    backup_id: String,
    database: String,
) -> std::result::Result<backup::DbRestore, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || backup::restore_database(&root, &meta_dir(&root)?, &backup_id, &database))
        .await
}

#[tauri::command]
fn install_preflight(
    state: State<'_, AppState>,
    dest: String,
    needed: Option<u64>,
    game_data: Option<String>,
) -> Preflight {
    if platform::flavor() == Flavor::Docker {
        return docker_install::preflight(&dest, game_data, needed, &state.registry);
    }
    // `needed` is the real size from the signed package list when the screen already has it; the real size is checked
    // again at install time either way.
    install::preflight(
        std::path::Path::new(&dest),
        needed.unwrap_or(6 * 1024 * 1024 * 1024),
        &state.registry,
    )
}

#[derive(Serialize)]
struct InstallRequirements {
    /// What is downloaded.
    download_bytes: u64,
    /// What the unpacked server takes on the drive (the downloaded parts are removed afterwards).
    unpacked_bytes: u64,
    version: String,
}

/// How big the server is, read from the package's signed list before anything is downloaded.
#[tauri::command]
async fn install_requirements(
    package: Option<String>,
) -> std::result::Result<InstallRequirements, UiError> {
    if platform::flavor() == Flavor::Docker {
        return docker_install::requirements(package).await;
    }
    blocking(move || {
        let (m, _) = coa_core::pkgsource::fetch_manifest(
            &installation_source(package)?,
            coa_core::signing::EMBEDDED_PUBLIC_KEY,
        )?;
        let archive = m
            .archive
            .ok_or_else(|| Error::InvalidManifest("no archive".into()))?;
        Ok(InstallRequirements {
            download_bytes: archive.parts.iter().map(|p| p.size).sum(),
            unpacked_bytes: archive.unpacked_size,
            version: m.version,
        })
    })
    .await
}

#[tauri::command]
async fn install_new(
    app: AppHandle,
    state: State<'_, AppState>,
    dest: String,
    package: Option<String>,
    game_data: Option<String>,
    copy_from: Option<String>,
) -> std::result::Result<InstalledServer, UiError> {
    let copy_from = copy_from.filter(|p| !p.trim().is_empty()).map(PathBuf::from);
    if platform::flavor() == Flavor::Docker {
        if copy_from.is_some() {
            return Err(Error::Invalid("A modular copy is made of a Windows server only.".into()).into());
        }
        // The plugin host is Windows-only for now: a Linux server keeps the classic core.
        return docker_install::install(app, state, dest, package, game_data)
            .await
            .map(|summary| InstalledServer { summary, preview_core_error: None, modules_off: Vec::new(), copied: None });
    }
    if let Some(old) = &copy_from {
        if !coa_core::modular_copy::is_classic(old) {
            return Err(Error::Invalid("This server already has the modular core; add it as it is.".into()).into());
        }
    }
    if std::fs::read_dir(&dest).is_ok_and(|mut d| d.next().is_some()) {
        return Err(Error::Invalid("Choose a new or empty folder for the server: this one already has files.".into()).into());
    }
    let cancel = Cancel::default();
    *state
        .install_cancel
        .lock()
        .map_err(|_| Error::Invalid("state poisoned".into()))? = Some(cancel.clone());
    let registry = Registry::at(data_dir().join("installs.json"));
    let dest_path = PathBuf::from(dest);
    // A package the owner picked by hand is installed as it is; otherwise the new server goes straight on to the
    // preview core. A new server starts with the bare core (modules are added later from the Modules page); a
    // modular copy of a classic server gets the modules its old server had switched on.
    let to_preview = package.as_deref().map_or(true, |p| p.trim().is_empty()) || copy_from.is_some();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let source = installation_source(package)?;
        let done = install::install_base(
            &install::Params {
                source,
                dest: dest_path.clone(),
                trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
                registry: &registry,
                cancel,
            },
            &|step| {
                let _ = app.emit("install-progress", step);
            },
        )?;
        let root = PathBuf::from(&done.path);
        let copied = match &copy_from {
            Some(old) => {
                let report = coa_core::modular_copy::copy_data(old, &root, &|what| {
                    let _ = app.emit(
                        "install-progress",
                        serde_json::json!({ "step": "Copying the old server's data", "percent": 100, "detail": what }),
                    );
                });
                match report {
                    Ok(r) => Some(r),
                    Err(e) => {
                        // Nothing of the old server was changed; take away the half-made copy (this call created it).
                        let _ = registry.unregister(&done.id);
                        let _ = coa_core::driver::run(&root, Verb::StopAll);
                        let _ = std::fs::remove_dir_all(&root);
                        if let Ok(meta) = metadata_dir_for(&root) {
                            let _ = std::fs::remove_dir_all(meta);
                        }
                        return Err(Error::Invalid(format!("The old server's data could not be copied: {e}")));
                    }
                }
            }
            None => {
                if to_preview {
                    coa_core::rebuild::decline_recommended(&root)?;
                }
                None
            }
        };
        let (preview_core_error, modules_off) = match to_preview {
            false => (None, Vec::new()),
            true => match install_preview_core(&app, &root) {
                Ok(off) => (None, off),
                Err(e) => (Some(e), Vec::new()),
            },
        };
        Ok::<_, Error>((done, preview_core_error, modules_off, copied))
    })
    .await;
    if let Ok(mut c) = state.install_cancel.lock() {
        *c = None;
    }
    let (done, preview_core_error, modules_off, copied) = result.map_err(|e| Error::Invalid(e.to_string()))??;
    Ok(InstalledServer {
        summary: summary(done.id, PathBuf::from(done.path)),
        preview_core_error,
        modules_off,
        copied,
    })
}

#[tauri::command]
fn is_classic_server(path: String) -> bool {
    coa_core::modular_copy::is_classic(Path::new(&path))
}

#[derive(Serialize)]
struct InstalledServer {
    #[serde(flatten)]
    summary: ServerSummary,
    /// Set when the server is installed but stayed on the classic core: why the preview core did not go on.
    preview_core_error: Option<String>,
    /// Modules that did not build for the preview core and were switched off, with the reason.
    modules_off: Vec<String>,
    /// For a modular copy of a classic server: what was copied.
    copied: Option<coa_core::modular_copy::CopyReport>,
}

/// Put the preview core on a server that was just installed from the stable image: the same signed update every
/// existing server takes, including building the recommended modules as plugins. A failure leaves the classic core
/// (the update rolls itself back) and is returned as text for the install screen; on success it returns the
/// modules that did not build and were switched off (the core goes on regardless).
fn install_preview_core(app: &AppHandle, root: &Path) -> std::result::Result<Vec<String>, String> {
    let run = || -> Result<update::Outcome> {
        let (dir, meta) = install_meta(root)?;
        let src = coa_core::update_route::select(
            &meta,
            update_source(None)?,
            coa_core::signing::EMBEDDED_PUBLIC_KEY,
        )?
        .source;
        let env = update::RepackEnv {
            root,
            meta_dir: &dir,
        };
        update::apply(
            &update::Params {
                root,
                meta_dir: &dir,
                source: src,
                trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
                cancel: Cancel::default(),
                resolutions: BTreeMap::new(),
                env: &env,
                fail_after_ops: None,
            },
            &|step, percent| {
                let _ = app.emit(
                    "install-progress",
                    serde_json::json!({ "step": "Installing the modular core", "percent": percent, "detail": step }),
                );
            },
        )
    };
    // Unused ports while it runs: the server it was copied from (or any other) may be running on the same ones.
    let outcome = coa_core::install::with_scratch_ports(root, run).map_err(|e| e.to_string())?;
    if outcome.txn.state != update::State::Committed {
        return Err(format!(
            "the preview core update stopped in state {:?}; open Updates to finish or roll it back",
            outcome.txn.state
        ));
    }
    Ok(outcome
        .modules
        .iter()
        .filter_map(|m| match m {
            coa_core::rebuild::ModuleRebuild::Disabled { id, reason, .. } => Some(format!("{id}: {reason}")),
            _ => None,
        })
        .collect())
}

#[tauri::command]
fn cancel_install(state: State<'_, AppState>) {
    if let Ok(c) = state.install_cancel.lock() {
        if let Some(c) = c.as_ref() {
            c.cancel();
        }
    }
}

#[tauri::command]
async fn create_account(
    state: State<'_, AppState>,
    id: String,
    username: String,
    password: String,
    administrator: bool,
) -> std::result::Result<(), UiError> {
    let _guard = BusyGuard::acquire(&state, &id)?;
    let root = path_of(&state, &id)?;
    blocking(move || {
        coa_core::accounts::create(&root, &username, &password, administrator)?;
        tracing::info!(%username, administrator, "account created");
        Ok(())
    })
    .await
}

#[tauri::command]
async fn realmlist_profiles(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::realmlist::View, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        Ok(coa_core::realmlist::view(
            &dir,
            linked_client(&root)?.as_deref(),
        ))
    })
    .await
}

#[tauri::command]
async fn realmlist_save(
    state: State<'_, AppState>,
    id: String,
    profile_id: Option<String>,
    name: String,
    data: String,
) -> std::result::Result<coa_core::realmlist::Profile, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::realmlist::save_profile(
            &dir,
            linked_client(&root)?.as_deref(),
            profile_id.as_deref(),
            &name,
            &data,
        )
    })
    .await
}

#[tauri::command]
async fn realmlist_delete(
    state: State<'_, AppState>,
    id: String,
    profile_id: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::realmlist::delete_profile(&dir, linked_client(&root)?.as_deref(), &profile_id)
    })
    .await
}

/// Write the chosen realmlist into the linked game client.
#[tauri::command]
async fn realmlist_activate(
    state: State<'_, AppState>,
    id: String,
    profile_id: String,
) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        let client = linked_client(&root)?.ok_or_else(|| {
            Error::Invalid("No game client is set up for this server yet.".into())
        })?;
        let changed = coa_core::realmlist::activate(&dir, &client, &profile_id)?;
        tracing::info!(profile = %profile_id, files = changed.len(), "realmlist switched");
        Ok(changed)
    })
    .await
}

#[tauri::command]
async fn modules_list(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<coa_core::modules::ModuleView>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || Ok(coa_core::modules::list(&root))).await
}

#[tauri::command]
async fn realm_profiles(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::realms::View, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::realms::view(&root)).await
}

#[tauri::command]
async fn realm_select(
    state: State<'_, AppState>,
    id: String,
    mode: coa_core::realms::Mode,
    restart: bool,
) -> std::result::Result<coa_core::realms::View, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    let result = blocking(move || {
        if restart {
            let out = driver::run(&root, Verb::StopAll)?;
            if !out.ok {
                return Err(Error::Invalid(out.output));
            }
        }
        let view = coa_core::realms::select(&root, mode)?;
        let (dir, meta) = install_meta(&root)?;
        if let Some(client) = meta.client_path {
            coa_core::client::sync_realm(std::path::Path::new(&client), &dir, view.active)?;
        }
        if restart {
            let out = driver::run(&root, Verb::StartAll)?;
            if !out.ok {
                return Err(Error::Invalid(format!(
                    "Realm selected, but startup failed: {}",
                    out.output
                )));
            }
        }
        Ok(view)
    })
    .await;
    result
}

#[tauri::command]
async fn realm_simultaneous(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> std::result::Result<coa_core::realms::View, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || coa_core::multiworld::set_enabled(&root, enabled)).await
}

#[tauri::command]
async fn realm_with_reborn(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> std::result::Result<coa_core::realms::View, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || coa_core::multiworld::set_reborn_enabled(&root, enabled)).await
}

#[tauri::command]
async fn check_database(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<coa_core::repair::DatabaseCheck>, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || coa_core::repair::check(&root, &meta_dir(&root)?)).await
}

#[tauri::command]
async fn repair_server(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::repair::Report, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let dir = meta_dir(&root)?;
        coa_core::repair::run(
            &root,
            &dir,
            &package_source(None),
            &update_source(None)?,
            coa_core::signing::EMBEDDED_PUBLIC_KEY,
            &|step, percent| {
                let _ = app.emit(
                    "repair-progress",
                    serde_json::json!({ "id": id, "step": step, "percent": percent }),
                );
            },
        )
    })
    .await
}

#[tauri::command]
async fn module_set_enabled(
    state: State<'_, AppState>,
    id: String,
    module: String,
    enabled: bool,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        // 1. If registered as dynamic plugin in .manager/plugins.json, synchronize plugin state
        if let Ok(ref r) = coa_core::plugins::load_registry(&root) {
            let plug_key = if r.plugins.contains_key(&module) {
                Some(module.clone())
            } else if r.plugins.contains_key(&format!("coa-{}", module)) {
                Some(format!("coa-{}", module))
            } else if r.plugins.contains_key(&format!("mod-{}", module)) {
                Some(format!("mod-{}", module))
            } else if r.plugins.contains_key(&format!("mod-coa-{}", module)) {
                Some(format!("mod-coa-{}", module))
            } else {
                None
            };

            if let Some(key) = plug_key {
                let _ = coa_core::plugins::set_enabled(&root, &key, enabled);
            }
        }

        // 2. Synchronize module .conf file / core setting
        let dir = match install_meta(&root) {
            Ok((dir, _)) => dir,
            Err(_) => {
                let fallback = root.join(".manager");
                let _ = std::fs::create_dir_all(&fallback);
                fallback
            }
        };
        coa_core::modules::set_enabled(&root, &dir, &module, enabled)
    })
    .await
}

#[tauri::command]
async fn plugins_repair(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || coa_core::plugins::repair(&root)).await
}

#[tauri::command]
async fn custom_races_status(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Option<coa_core::custom_races::ClientPatchStatus>, UiError> {
    let ctx = client_context(&state, &id)?;
    blocking(move || {
        let client = ctx.linked();
        Ok(client
            .as_deref()
            .map(coa_core::custom_races::client_patch_status))
    })
    .await
}

#[tauri::command]
async fn race_availability(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::race_availability::Availability, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::race_availability::status(&root)).await
}

#[tauri::command]
async fn race_availability_save(
    state: State<'_, AppState>,
    id: String,
    catalog_sha256: String,
    realm: coa_core::realms::Mode,
    disabled_race_ids: Vec<u32>,
) -> std::result::Result<coa_core::race_availability::Availability, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::race_availability::save(&root, &dir, &catalog_sha256, realm, &disabled_race_ids)
    })
    .await
}

#[tauri::command]
async fn custom_races_install(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<(), UiError> {
    let ctx = client_context(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    let cancel = begin_client_job(&state)?;
    let sid = id.clone();
    let result = blocking(move || {
        let client = ctx.require()?;
        coa_core::custom_races::install_client_patches(&client, &cancel, &|step, percent| {
            let _ = app.emit(
                "custom-races-progress",
                serde_json::json!({
                    "id": sid,
                    "step": step,
                    "percent": percent,
                }),
            );
        })?;
        let enabled = ctx.root.as_ref().is_none_or(|root| {
            coa_core::modules::list(root)
                .iter()
                .any(|module| module.id == "custom-races" && module.enabled)
        });
        coa_core::custom_races::set_client_patch_enabled(&client, enabled)
    })
    .await;
    end_client_job(&state);
    result
}

#[tauri::command]
async fn custom_races_uninstall(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<(), UiError> {
    let ctx = client_context(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || coa_core::custom_races::uninstall_client_patches(&ctx.require()?)).await
}

#[tauri::command]
async fn custom_races_toggle_client(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let client = linked_client(&root)?
            .ok_or_else(|| Error::Invalid("No game client is linked to this server.".into()))?;
        coa_core::custom_races::set_client_patch_enabled(&client, enabled)
    })
    .await
}

#[tauri::command]
async fn module_settings(
    state: State<'_, AppState>,
    id: String,
    module: String,
) -> std::result::Result<Vec<coa_core::modules::Setting>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::modules::settings(&root, &module)).await
}

#[tauri::command]
async fn module_save_settings(
    state: State<'_, AppState>,
    id: String,
    module: String,
    changes: BTreeMap<String, String>,
) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::modules::save_settings(&root, &dir, &module, &changes)
    })
    .await
}

#[tauri::command]
async fn all_settings(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<coa_core::allsettings::Item>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::allsettings::list(&root)).await
}

#[tauri::command]
async fn all_settings_save(
    state: State<'_, AppState>,
    id: String,
    changes: BTreeMap<String, String>,
) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::allsettings::save(&root, &dir, &changes)
    })
    .await
}

#[tauri::command]
async fn list_accounts(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<coa_core::accounts::AccountInfo>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::accounts::list(&root)).await
}

#[tauri::command]
async fn account_set_password(
    state: State<'_, AppState>,
    id: String,
    name: String,
    password: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        Ra::connect(&root)?.set_account_password(&name, &password)?;
        tracing::info!(%name, "account password changed");
        Ok(())
    })
    .await
}

#[tauri::command]
async fn account_set_access(
    state: State<'_, AppState>,
    id: String,
    name: String,
    level: u8,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        Ra::connect(&root)?.set_account_access(&name, level)?;
        tracing::info!(%name, level, "account access level changed");
        Ok(())
    })
    .await
}

#[tauri::command]
async fn account_rename(
    state: State<'_, AppState>,
    id: String,
    name: String,
    new_name: String,
    password: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let mut ra = Ra::connect(&root)?;
        coa_core::accounts::rename(&root, &mut ra, &name, &new_name, &password)
    })
    .await
}

#[tauri::command]
async fn account_delete(
    state: State<'_, AppState>,
    id: String,
    name: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let mut ra = Ra::connect(&root)?;
        coa_core::accounts::delete(&root, &mut ra, &name)
    })
    .await
}

fn install_meta(root: &std::path::Path) -> Result<(PathBuf, InstallMeta)> {
    let dir = meta_dir(root)?;
    let (_, meta) = MetaDir::open(&dir)?;
    Ok((dir, meta))
}

#[tauri::command]
async fn check_update(
    state: State<'_, AppState>,
    id: String,
    source: Option<String>,
    background: Option<bool>,
) -> std::result::Result<update::Preview, UiError> {
    let root = path_of(&state, &id)?;

    // The check that repeats every few minutes must not start and stop the database of a stopped server.
    let access = if background.unwrap_or(false) {
        update::DatabaseAccess::OnlyIfRunning
    } else {
        update::DatabaseAccess::Start
    };
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        let selection = coa_core::update_route::select(
            &meta,
            update_source(source)?,
            coa_core::signing::EMBEDDED_PUBLIC_KEY,
        )?;
        let mut preview = update::preview_with(
            &root,
            &meta,
            &selection.source,
            coa_core::signing::EMBEDDED_PUBLIC_KEY,
            &Default::default(),
            access,
        )?;
        preview.latest_version = selection.latest_version;
        Ok(preview)
    })
    .await
}

#[tauri::command]
fn pending_update(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Option<update::Txn>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(update::pending_checked(&meta_dir(&root)?)?)
}

#[tauri::command]
async fn apply_update(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    source: Option<String>,
    resolutions: BTreeMap<String, Resolution>,
) -> std::result::Result<update::Outcome, UiError> {
    let root = path_of(&state, &id)?;

    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, meta) = install_meta(&root)?;
        let src = coa_core::update_route::select(
            &meta,
            update_source(source)?,
            coa_core::signing::EMBEDDED_PUBLIC_KEY,
        )?
        .source;
        let (manifest, _) =
            coa_core::pkgsource::fetch_manifest(&src, coa_core::signing::EMBEDDED_PUBLIC_KEY)?;
        update::validate_candidate(&meta, &manifest)?;
        // Files cannot be replaced while the server runs: stop it first (gracefully), like the Stop button.
        let observed = coa_core::process::observe(&root, &layout::read_ports(&root));
        if observed.world.state != coa_core::process::ServiceState::Stopped
            || observed.auth.state != coa_core::process::ServiceState::Stopped
            || coa_core::multiworld::is_running(&root)
        {
            let out = driver::run(&root, Verb::StopAll)?;
            if !out.ok {
                return Err(Error::Invalid(
                    "The server could not be stopped, so the update was not started.".into(),
                ));
            }
        }
        let env = update::RepackEnv {
            root: &root,
            meta_dir: &dir,
        };
        update::apply(
            &update::Params {
                root: &root,
                meta_dir: &dir,
                source: src,
                trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
                cancel: Cancel::default(),
                resolutions,
                env: &env,
                fail_after_ops: None,
            },
            &|step, percent| {
                let _ = app.emit(
                    "update-progress",
                    serde_json::json!({ "step": step, "percent": percent }),
                );
            },
        )
    })
    .await
}

#[tauri::command]
async fn rollback_update(
    state: State<'_, AppState>,
    id: String,
    txn: String,
) -> std::result::Result<update::Txn, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let dir = meta_dir(&root)?;
        let env = update::RepackEnv {
            root: &root,
            meta_dir: &dir,
        };
        update::rollback(&root, &dir, &txn, &env)
    })
    .await
}

#[tauri::command]
async fn retry_update_validation(
    state: State<'_, AppState>,
    id: String,
    txn: String,
) -> std::result::Result<update::Txn, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let dir = meta_dir(&root)?;
        let pending = update::load(&dir, &txn)?;
        let source = coa_core::pkgsource::Source::Url(format!(
            "https://github.com/Corfirean/coa-server-build/releases/download/server-{}/",
            pending.to_version
        ));
        let env = update::RepackEnv {
            root: &root,
            meta_dir: &dir,
        };
        update::retry_validation(
            &root,
            &dir,
            &txn,
            &source,
            coa_core::signing::EMBEDDED_PUBLIC_KEY,
            &env,
        )
    })
    .await
}

#[tauri::command]
async fn get_population(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Option<coa_core::population::Population>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(tauri::async_runtime::spawn_blocking(move || {
        let o = coa_core::process::observe(&root, &layout::read_ports(&root));
        if o.mysql.state != coa_core::process::ServiceState::Running {
            return None;
        }
        coa_core::population::query(&root).ok()
    })
    .await
    .unwrap_or(None))
}

#[derive(Serialize)]
struct CompanionAction {
    /// Bots affected (cancelled from the queue, or logged out).
    count: u32,
}

#[tauri::command]
async fn companions_stop_spawning(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<CompanionAction, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        Ok(CompanionAction {
            count: Ra::connect(&root)?.cancel_spawning()?,
        })
    })
    .await
}

#[tauri::command]
async fn companions_take_offline(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<CompanionAction, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let mut ra = Ra::connect(&root)?;
        // Anything still waiting to be created must not come back right after.
        let _ = ra.cancel_spawning();
        Ok(CompanionAction {
            count: ra.despawn_all()?,
        })
    })
    .await
}

/// Log `count` randomly chosen online companions out to lower the load. Uses the per-bot command that every server
/// build has, so it works on older builds too.
#[tauri::command]
async fn companions_despawn_some(
    state: State<'_, AppState>,
    id: String,
    count: u32,
) -> std::result::Result<CompanionAction, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        if !(1..=5000).contains(&count) {
            return Err(Error::Invalid("Choose a number between 1 and 5000.".into()));
        }
        let prefix = coa_core::population::bot_account_prefix(&root).to_uppercase();
        let db = coa_core::db::Db::from_repack(&root, coa_core::db::Account::App)?;
        let rows = db.query(&format!(
            "SELECT c.guid FROM acore_characters.characters c JOIN acore_auth.account a ON a.id=c.account              WHERE c.online=1 AND UPPER(a.username) LIKE '{prefix}%' ORDER BY RAND() LIMIT {count};"
        ))?;
        let guids: Vec<u64> = rows.lines().filter_map(|l| l.trim().parse().ok()).collect();
        let mut ra = Ra::connect(&root)?;
        let mut done = 0;
        for g in guids {
            if ra.despawn_bot(g).unwrap_or(false) {
                done += 1;
            }
        }
        Ok(CompanionAction { count: done })
    })
    .await
}

/// Delete every companion for good. A recovery point of the characters and accounts is saved first.
#[tauri::command]
async fn companions_delete_all(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<CompanionAction, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        backup::create(
            &root,
            &meta,
            Kind::Quick,
            Trigger::BeforeDangerousChange,
            Some("before deleting all companions".into()),
            &|step| {
                let _ = app.emit("backup-progress", step);
            },
        )?;
        let before = coa_core::population::query(&root)
            .map(|p| p.bots_total)
            .unwrap_or(0);
        let mut ra = Ra::connect(&root)?;
        let _ = ra.cancel_spawning();
        ra.purge_all()?;
        Ok(CompanionAction { count: before })
    })
    .await
}

#[tauri::command]
async fn get_performance(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Option<coa_core::ra::Performance>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(tauri::async_runtime::spawn_blocking(move || {
        let o = coa_core::process::observe(&root, &layout::read_ports(&root));
        if o.world.state != coa_core::process::ServiceState::Running {
            return None;
        }
        Ra::connect(&root)
            .and_then(|mut r| r.performance())
            .ok()
            .flatten()
    })
    .await
    .unwrap_or(None))
}

#[derive(Serialize)]
struct CompanionSizes {
    hardware: coa_core::population::Hardware,
    sizes: Vec<coa_core::population::SizeOption>,
}

#[tauri::command]
fn companion_sizes() -> CompanionSizes {
    let hardware = coa_core::population::hardware();
    let sizes = coa_core::population::sizes(&hardware);
    CompanionSizes { hardware, sizes }
}

#[derive(Serialize)]
struct CompanionsResult {
    spawned: Option<String>,
    /// Created offline with their equipment (server was stopped and the batch was large).
    created: Option<u32>,
    /// Companions that existed before this request (for a progress bar while the server creates the new ones).
    baseline: Option<u32>,
}

/// Turn on automatic bot login for `count` bots and, if the server is running, ask it to create them.
#[tauri::command]
async fn add_companions(
    state: State<'_, AppState>,
    id: String,
    count: u32,
) -> std::result::Result<CompanionsResult, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        if !(1..=2000).contains(&count) {
            return Err(Error::Invalid(
                "Choose between 1 and 2000 companions.".into(),
            ));
        }
        let meta = meta_dir(&root)?;
        let mut changes = BTreeMap::new();
        changes.insert("CoaBots.AutoLoginOnStartup".to_string(), Value::Bool(true));
        // Every companion, old and new, should come back at the next start, so the start-up limit only ever grows here.
        let existing_max = config::load(&root, Scope::Bots)
            .ok()
            .and_then(|v| {
                v.settings
                    .iter()
                    .find(|s| s.meta.key == "CoaBots.AutoLogin.MaxCount")
                    .and_then(|s| s.value.as_u64())
            })
            .unwrap_or(0) as u32;
        let already = coa_core::population::query(&root)
            .map(|p| p.bots_total)
            .unwrap_or(0);
        changes.insert(
            "CoaBots.AutoLogin.MaxCount".to_string(),
            Value::from(existing_max.max(already + count).min(5000)),
        );
        config::save(&root, &meta, Scope::Bots, &changes)?;
        let o = coa_core::process::observe(&root, &layout::read_ports(&root));
        // Large batches while the server is stopped: create them in the database right now, fully equipped.
        if o.world.state == coa_core::process::ServiceState::Stopped
            && o.auth.state == coa_core::process::ServiceState::Stopped
            && count >= coa_core::companions::OFFLINE_MIN
            && coa_core::companions::offline_factory(&root).is_some()
        {
            let started_db = o.mysql.state != coa_core::process::ServiceState::Running;
            if started_db {
                let out = driver::run(&root, Verb::StartMysql)?;
                if !out.ok {
                    return Err(Error::Invalid("The database could not be started.".into()));
                }
            }
            let made = (|| -> Result<()> {
                coa_core::companions::ensure_templates(&coa_core::db::Db::from_repack(
                    &root,
                    coa_core::db::Account::Admin,
                )?)?;
                coa_core::companions::offline_create(
                    &root,
                    &meta.join("logs").join("companions-offline.log"),
                    count,
                )?;
                Ok(())
            })();
            if started_db {
                let _ = driver::run(&root, Verb::StopAll);
            }
            made?;
            tracing::info!(count, "companions created offline");
            return Ok(CompanionsResult {
                spawned: None,
                created: Some(count),
                baseline: None,
            });
        }
        let mut baseline = None;
        let spawned = if o.world.state == coa_core::process::ServiceState::Running {
            // Older servers have no template characters to copy bots from: add them first.
            coa_core::companions::ensure_templates(&coa_core::db::Db::from_repack(
                &root,
                coa_core::db::Account::Admin,
            )?)?;
            baseline = coa_core::population::query(&root)
                .ok()
                .map(|p| p.bots_total);
            Some(Ra::connect(&root)?.spawn_bots(count)?)
        } else {
            // The server is stopped: creating bots needs it running, so remember the request and do it after the next start.
            coa_core::fsx::atomic_write_json(
                &meta.join("companions.pending.json"),
                &serde_json::json!({ "count": count }),
            )?;
            None
        };
        tracing::info!(count, spawned = spawned.is_some(), "companions requested");
        Ok(CompanionsResult {
            spawned,
            created: None,
            baseline,
        })
    })
    .await
}

const REMOTE_CLIENT_ID: &str = "@remote-client";

fn remote_dir() -> PathBuf {
    data_dir().join("remote-client")
}

#[tauri::command]
fn remote_connection() -> std::result::Result<coa_core::remote_client::Profile, UiError> {
    Ok(coa_core::remote_client::load(&remote_dir())?)
}

#[tauri::command]
fn remote_connect(
    state: State<'_, AppState>,
    host: String,
) -> std::result::Result<coa_core::remote_client::Profile, UiError> {
    let job = state.client_cancel.lock().unwrap();
    if job.is_some() {
        return Err(Error::Invalid("Wait for the client operation to finish.".into()).into());
    }
    let dir = remote_dir();
    let mut profile = coa_core::remote_client::load(&dir)?;
    let host = host.trim();
    if host.is_empty() {
        return Err(Error::Invalid("Enter the host's IP address or hostname.".into()).into());
    }
    profile.host = host.into();
    coa_core::remote_client::save(&dir, &profile)?;
    Ok(profile)
}

struct ClientContext {
    dir: PathBuf,
    path: Option<PathBuf>,
    source: Option<PathBuf>,
    host: String,
    root: Option<PathBuf>,
}

fn client_context(state: &AppState, id: &str) -> Result<ClientContext> {
    if id == REMOTE_CLIENT_ID {
        let dir = remote_dir();
        let profile = coa_core::remote_client::load(&dir)?;
        return Ok(ClientContext {
            dir,
            path: profile.client_path.map(PathBuf::from),
            source: None,
            host: profile.host,
            root: None,
        });
    }
    let root = path_of(state, id)?;
    let (dir, meta) = install_meta(&root)?;
    let source = coa_core::client::addon_source(&root);
    Ok(ClientContext {
        dir,
        path: meta.client_path.map(PathBuf::from),
        source,
        host: "127.0.0.1".into(),
        root: Some(root),
    })
}

impl ClientContext {
    fn linked(&self) -> Option<PathBuf> {
        self.path
            .clone()
            .filter(|p| coa_core::client::detect(p, None).is_some())
    }
    fn require(&self) -> Result<PathBuf> {
        self.linked()
            .ok_or_else(|| Error::Invalid("Set up a game client first.".into()))
    }
    fn link(&self, path: &str) -> Result<()> {
        if let Some(root) = &self.root {
            let (_, mut meta) = install_meta(root)?;
            meta.client_path = Some(path.into());
            coa_core::fsx::atomic_write_json(&self.dir.join("install.json"), &meta)
        } else {
            let mut profile = coa_core::remote_client::load(&self.dir)?;
            profile.client_path = Some(path.into());
            coa_core::remote_client::save(&self.dir, &profile)
        }
    }
}

fn client_of(root: &std::path::Path) -> Result<(PathBuf, InstallMeta, Option<PathBuf>)> {
    let (dir, meta) = install_meta(root)?;
    let client =
        meta.client_path.clone().map(PathBuf::from).ok_or_else(|| {
            Error::Invalid("No game client is set up for this server yet.".into())
        })?;
    Ok((dir, meta, Some(client)))
}

#[tauri::command]
fn client_graphics(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::client_graphics::Graphics, UiError> {
    Ok(coa_core::client_graphics::status(
        &client_context(&state, &id)?.require()?,
    )?)
}

#[tauri::command]
fn client_set_graphics(
    state: State<'_, AppState>,
    id: String,
    name: String,
    disabled: bool,
) -> std::result::Result<coa_core::client_graphics::Graphics, UiError> {
    let job = state.client_cancel.lock().unwrap();
    if job.is_some() {
        return Err(Error::Invalid("Wait for the client operation to finish.".into()).into());
    }
    Ok(coa_core::client_graphics::set(
        &client_context(&state, &id)?.require()?,
        &name,
        disabled,
    )?)
}

#[tauri::command]
fn client_info(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Option<coa_core::client::ClientInfo>, UiError> {
    let ctx = client_context(&state, &id)?;
    Ok(ctx
        .path
        .and_then(|p| coa_core::client::detect(&p, ctx.source.as_deref())))
}

#[tauri::command]
fn set_client(
    state: State<'_, AppState>,
    id: String,
    path: String,
) -> std::result::Result<coa_core::client::ClientInfo, UiError> {
    let job = state.client_cancel.lock().unwrap();
    if job.is_some() {
        return Err(Error::Invalid("Wait for the client operation to finish.".into()).into());
    }
    let ctx = client_context(&state, &id)?;
    let info = coa_core::client::detect(std::path::Path::new(&path), ctx.source.as_deref())
        .ok_or_else(|| Error::Invalid("This folder does not look like a game client (it needs Data and the game executable).".into()))?;
    ctx.link(&info.path)?;
    Ok(
        coa_core::client::detect(std::path::Path::new(&info.path), ctx.source.as_deref())
            .unwrap_or(info),
    )
}

#[tauri::command]
fn client_realmlist(
    state: State<'_, AppState>,
    id: String,
    host: String,
) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    let (dir, _, client) = client_of(&root)?;
    Ok(coa_core::client::set_realmlist(
        &client.unwrap(),
        &dir,
        &host,
    )?)
}

#[tauri::command]
fn client_install_addon(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    let (dir, _, client) = client_of(&root)?;
    let source = coa_core::client::addon_source(&root).ok_or_else(|| {
        Error::Invalid("This server package does not include the companion addon.".into())
    })?;
    Ok(coa_core::client::install_addon(
        &client.unwrap(),
        &dir,
        &source,
    )?)
}

#[derive(Serialize)]
struct ClientStatus {
    /// A usable client folder is linked to this server.
    linked: bool,
    /// The Manager keeps this folder up to date (it downloaded it or was asked to adopt it).
    managed: bool,
    installed_version: Option<String>,
    latest_version: Option<String>,
    latest_bytes: Option<u64>,
    /// A managed client is older than the published one (or its download never finished).
    update_available: bool,
}

fn linked_client(root: &std::path::Path) -> Result<Option<PathBuf>> {
    let (_, meta) = install_meta(root)?;
    Ok(meta
        .client_path
        .map(PathBuf::from)
        .filter(|p| coa_core::client::detect(p, None).is_some()))
}

/// Cheap: one small manifest request and the local state file; no game file is read.
#[tauri::command]
async fn client_status(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<ClientStatus, UiError> {
    let ctx = client_context(&state, &id)?;
    blocking(move || {
        let client = ctx.linked();
        let latest = coa_core::clientdl::fetch_latest().ok();
        let local = client.as_deref().map(coa_core::clientdl::local);
        let managed = local.as_ref().map(|l| l.managed).unwrap_or(false);
        let installed = local.and_then(|l| l.version);
        let update_available = managed
            && latest
                .as_ref()
                .map(|l| installed.as_deref() != Some(l.version.as_str()))
                .unwrap_or(false);
        Ok(ClientStatus {
            linked: client.is_some(),
            managed,
            installed_version: installed,
            latest_bytes: latest.as_ref().map(|l| l.total_bytes()),
            latest_version: latest.map(|l| l.version),
            update_available,
        })
    })
    .await
}

#[derive(Serialize)]
struct ClientDownloadCheck {
    needed_bytes: u64,
    free_bytes: u64,
    version: String,
    /// Where the client would go.
    dest: String,
}

const CLIENT_FOLDER: &str = "CoA Client";

#[tauri::command]
async fn client_download_check(
    parent: String,
) -> std::result::Result<ClientDownloadCheck, UiError> {
    blocking(move || {
        let latest = coa_core::clientdl::fetch_latest()?;
        let dest = PathBuf::from(&parent).join(CLIENT_FOLDER);
        Ok(ClientDownloadCheck {
            needed_bytes: latest.total_bytes(),
            free_bytes: coa_core::fsx::free_space(std::path::Path::new(&parent))?,
            version: latest.version,
            dest: dest.to_string_lossy().into_owned(),
        })
    })
    .await
}

/// Compare the linked client with the published one. Hashes files only where the recorded state cannot vouch for them.
#[tauri::command]
async fn client_plan(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::clientdl::Plan, UiError> {
    let ctx = client_context(&state, &id)?;
    let cancel = begin_client_job(&state)?;
    let result = blocking(move || {
        let client = ctx.require()?;
        let manifest = coa_core::clientdl::fetch_latest()?;
        let current = coa_core::clientdl::load_state(&client).unwrap_or_default();
        coa_core::clientdl::plan(&client, &manifest, &current, &cancel, &|s| {
            let _ = app.emit("client-progress", s);
        })
    })
    .await;
    end_client_job(&state);
    result
}

/// Bring the linked client to the published version. `keep_modified` leaves files the player changed alone.
#[tauri::command]
async fn client_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    keep_modified: bool,
) -> std::result::Result<(), UiError> {
    let ctx = client_context(&state, &id)?;
    let cancel = begin_client_job(&state)?;
    let result = blocking(move || {
        let client = ctx.require()?;
        run_client_sync(&app, &client, keep_modified, &cancel)?;
        if ctx.root.is_none() && !ctx.host.is_empty() {
            coa_core::client::set_realmlist(&client, &ctx.dir, &ctx.host)?;
        }
        Ok(())
    })
    .await;
    end_client_job(&state);
    result
}

fn run_client_sync(
    app: &AppHandle,
    client: &std::path::Path,
    keep_modified: bool,
    cancel: &Cancel,
) -> Result<()> {
    let emit = |s: coa_core::clientdl::Step| {
        let _ = app.emit("client-progress", s);
    };
    let manifest = coa_core::clientdl::fetch_latest()?;
    let current = coa_core::clientdl::load_state(client).unwrap_or_default();
    let plan = coa_core::clientdl::plan(client, &manifest, &current, cancel, &emit)?;
    coa_core::download::check_url(coa_core::clientdl::OBJECTS_URL)?;
    let transport = coa_core::download::HttpTransport::new()?;
    coa_core::clientdl::apply(
        &coa_core::clientdl::Apply {
            client,
            manifest: &manifest,
            plan: &plan,
            keep_modified,
            transport: &transport,
            objects_url: coa_core::clientdl::OBJECTS_URL,
            cancel,
        },
        &emit,
    )?;
    tracing::info!(version = %manifest.version, downloaded = plan.items.len(), "client brought up to date");
    Ok(())
}

/// Download the client into `<parent>/CoA Client`, then link it to a host or player profile.
#[tauri::command]
async fn client_download(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    parent: String,
) -> std::result::Result<coa_core::client::ClientInfo, UiError> {
    let ctx = client_context(&state, &id)?;
    let cancel = begin_client_job(&state)?;
    let result = blocking(move || {
        let parent = PathBuf::from(&parent);
        if !parent.is_dir() {
            return Err(Error::Invalid(
                "Choose an existing folder to put the game client in.".into(),
            ));
        }
        let dest = parent.join(CLIENT_FOLDER);
        let resuming = coa_core::clientdl::load_state(&dest).is_some();
        let non_empty = std::fs::read_dir(&dest)
            .map(|mut r| r.next().is_some())
            .unwrap_or(false);
        if non_empty && !resuming {
            return Err(Error::Invalid(format!(
                "{} already exists and is not an unfinished download. Choose another folder.",
                dest.display()
            )));
        }
        std::fs::create_dir_all(&dest)?;
        run_client_sync(&app, &dest, false, &cancel)?;
        let info = coa_core::client::detect(&dest, ctx.source.as_deref())
            .ok_or_else(|| Error::Invalid("The downloaded client looks incomplete.".into()))?;
        ctx.link(&info.path)?;
        if !ctx.host.is_empty() {
            coa_core::client::set_realmlist(&dest, &ctx.dir, &ctx.host)?;
        }
        if let Some(src) = &ctx.source {
            coa_core::client::install_addon(&dest, &ctx.dir, src)?;
        }
        Ok(coa_core::client::detect(&dest, ctx.source.as_deref()).unwrap_or(info))
    })
    .await;
    end_client_job(&state);
    result
}

#[tauri::command]
fn client_cancel(state: State<'_, AppState>) {
    if let Ok(c) = state.client_cancel.lock() {
        if let Some(c) = c.as_ref() {
            c.cancel();
        }
    }
}

fn begin_client_job(state: &AppState) -> std::result::Result<Cancel, UiError> {
    let mut slot = state
        .client_cancel
        .lock()
        .map_err(|_| Error::Invalid("state poisoned".into()))?;
    if slot.is_some() {
        return Err(Error::Invalid(
            "The game client is already being checked or downloaded.".into(),
        )
        .into());
    }
    let cancel = Cancel::default();
    *slot = Some(cancel.clone());
    Ok(cancel)
}

fn end_client_job(state: &AppState) {
    if let Ok(mut c) = state.client_cancel.lock() {
        *c = None;
    }
}

/// Launch the game client independently of the local server.
#[tauri::command]
async fn play(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<DriverOutcome, UiError> {
    if id == REMOTE_CLIENT_ID {
        let ctx = client_context(&state, &id)?;
        let client = ctx.require()?;
        return blocking(move || {
            if ctx.host.is_empty() {
                return Err(Error::Invalid(
                    "Enter the host's IP address or hostname first.".into(),
                ));
            }
            if coa_core::client::is_running(&client) {
                return Err(Error::Invalid("The game is already running.".into()));
            }
            coa_core::client::begin_realmlist_override(&client, &ctx.dir, &ctx.host)?;
            coa_core::client::launch(&client)?;
            let client_c = client.clone();
            let meta_c = ctx.dir.clone();
            std::thread::Builder::new()
                .name("client-realmlist-watcher".into())
                .spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(3000));
                    while coa_core::client::is_running(&client_c) {
                        std::thread::sleep(std::time::Duration::from_millis(1000));
                    }
                    let _ = coa_core::client::revert_realmlist_override(&meta_c);
                })
                .ok();
            Ok(DriverOutcome {
                ok: true,
                exit_code: None,
                code: None,
                human: None,
                output: String::new(),
            })
        })
        .await;
    }
    let root = path_of(&state, &id)?;
    let (dir, _, client) = client_of(&root)?;
    let client = client.unwrap();
    if coa_core::client::is_running(&client) {
        return Err(Error::Invalid(
            "Close the game client before pressing Play so its selected realm can be updated."
                .into(),
        )
        .into());
    }
    blocking(move || {
        let mode = coa_core::realms::state(&root)?.active;
        if !coa_core::client::sync_realm(&client, &dir, mode)? {
            return Err(Error::Invalid(
                "Close the game client before pressing Play so its selected realm can be updated."
                    .into(),
            ));
        }
        let existing = coa_core::glue_config::read_glue_config(&client).unwrap_or_default();
        if existing.is_empty() {
            let _ = coa_core::glue_config::prefill_login(&client, "local", "local");
        }
        coa_core::client::launch(&client)?;
        Ok(DriverOutcome {
            ok: true,
            exit_code: None,
            code: None,
            human: None,
            output: String::new(),
        })
    })
    .await
}

#[derive(Serialize)]
struct FriendsStatus {
    settings: coa_core::friends::Settings,
    lan_ip: Option<String>,
    lan_addresses: Vec<coa_core::net::LanAddress>,
    exposure: Vec<coa_core::net::Exposure>,
    /// The configuration lets other computers reach the login and world servers.
    servers_open: bool,
    firewall: Option<coa_core::firewall::Status>,
    tailscale: coa_core::net::Tailscale,
    server_running: bool,
    auth_port: u16,
    world_port: u16,
    secondary_world_port: Option<u16>,
    reborn_world_port: Option<u16>,
}

/// Open one of a few known help pages in the default browser. Anything else is refused, so a page can never ask the
/// Manager to launch an arbitrary address.
#[tauri::command]
fn open_link(url: String) -> std::result::Result<(), UiError> {
    const ALLOWED: &[&str] = &[
        "https://tailscale.com/",
        "https://login.tailscale.com/",
        "https://portforward.com/",
        "https://github.com/Corfirean/",
    ];
    // A prefilled "new issue" page of one of the places a report can go (the Manager, Companions, SQUID Playerbots): the
    // address must be exactly that page, and its query may carry `&` between the (percent-encoded) title and text.
    let issue = coa_core::report::is_new_issue_url(&url);
    let chars_ok = url
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/:._-?=#%".contains(c) || (issue && c == '&'));
    // the GitHub page of a module that is in the bundled catalog (some are not ours)
    let catalog = coa_core::modules::catalog().iter().any(|e| e.repo == url);
    if !(issue || catalog || ALLOWED.iter().any(|p| url.starts_with(p)))
        || !chars_ok
        || url.len() > 12_000
    {
        return Err(Error::Invalid("That link is not allowed.".into()).into());
    }
    tauri_plugin_opener::open_url(&url, None::<&str>).map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(())
}

#[derive(Serialize)]
struct ReportContext {
    manager_version: String,
    windows: String,
    /// "new" for a server the Manager installed, "imported" for one that was added.
    install_kind: String,
    server_version: Option<String>,
    /// Where the form should start: the bot system that is switched on, else the Manager.
    suggested_target: String,
    /// The release of whichever bot system is on, for a report that goes to its repository.
    bots_version: Option<String>,
}

/// The places a report can go, each with its GitHub repository.
#[tauri::command]
fn report_targets() -> Vec<coa_core::report::Target> {
    coa_core::report::targets()
}

/// The distribution on Linux ("Linux (Arch Linux)"), from `/etc/os-release`; empty if it cannot be read.
#[cfg(not(windows))]
fn windows_version() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.strip_prefix("PRETTY_NAME=")
                    .map(|v| v.trim_matches('"').to_string())
            })
        })
        .map(|name| format!("Linux ({name})"))
        .unwrap_or_default()
}

/// "Windows 11 (build 26200)" from `ver`; empty if it cannot be read.
#[cfg(windows)]
fn windows_version() -> String {
    let mut cmd = std::process::Command::new("cmd.exe");
    cmd.args(["/C", "ver"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let build: Option<u32> = out
        .split("Version")
        .nth(1)
        .and_then(|v| v.trim().trim_end_matches(']').split('.').nth(2))
        .and_then(|b| b.trim().parse().ok());
    match build {
        Some(b) if b >= 22000 => format!("Windows 11 (build {b})"),
        Some(b) => format!("Windows 10 (build {b})"),
        None => String::new(),
    }
}

/// What the problem report fills in on its own. Nothing here identifies the person or the machine.
#[tauri::command]
async fn report_context(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<ReportContext, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        let bots = coa_core::report::bots(&root);
        let companions = [
            meta.bots.version.clone(),
            meta.bots
                .commit
                .as_ref()
                .map(|c| c.chars().take(8).collect()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<String>>()
        .join(" В· ");
        Ok(ReportContext {
            manager_version: coa_core::MANAGER_VERSION.to_string(),
            windows: windows_version(),
            install_kind: if meta.kind == coa_core::registry::InstallKind::New {
                "new".into()
            } else {
                "imported".into()
            },
            server_version: meta.core.version.clone(),
            suggested_target: bots.suggested.to_string(),
            bots_version: if bots.suggested == "squid" {
                bots.squid_version
            } else if bots.suggested == "companions" && !companions.is_empty() {
                Some(companions)
            } else {
                None
            },
        })
    })
    .await
}

#[tauri::command]
async fn friends_status(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<FriendsStatus, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        let ports = layout::read_ports(&root);
        let automatic = coa_core::net::lan_ip().filter(|ip| coa_core::net::is_lan_address(*ip));
        Ok(FriendsStatus {
            settings: coa_core::friends::load(&meta),
            lan_ip: automatic.map(|a| a.to_string()),
            lan_addresses: coa_core::net::lan_addresses(automatic).unwrap_or_default(),
            exposure: if coa_core::docker::is_docker(&root) {
                coa_core::docker::exposure(&root, &ports)
            } else {
                coa_core::net::exposure(&ports)
            },
            servers_open: coa_core::friends::bind_is_open(&root),
            // The Windows firewall exists only on Windows; on Linux Docker publishes the ports itself.
            firewall: (platform::flavor() == Flavor::Repack).then(coa_core::firewall::status),
            tailscale: coa_core::net::tailscale(),
            server_running: coa_core::process::observe(&root, &ports).world.state
                == coa_core::process::ServiceState::Running,
            auth_port: ports.auth,
            world_port: ports.world,
            secondary_world_port: coa_core::realms::state(&root)?
                .secondary_world_port
                .filter(|_| coa_core::realms::state(&root).is_ok_and(|s| s.simultaneous)),
            reborn_world_port: coa_core::realms::state(&root)?
                .reborn_world_port
                .filter(|_| coa_core::realms::state(&root).is_ok_and(|s| s.with_reborn)),
        })
    })
    .await
}

#[derive(Serialize)]
struct InternetCheck {
    public_ip: Option<String>,
    router_ip: Option<String>,
    reachability: coa_core::net::Reachability,
    router_found: bool,
}

/// Talks to an outside address service and to the router. Only runs when the user presses the button.
#[tauri::command]
async fn friends_check_internet() -> std::result::Result<InternetCheck, UiError> {
    blocking(move || {
        let public = coa_core::net::public_ip().ok();
        let gw = coa_core::upnp::discover();
        let router = gw
            .as_ref()
            .and_then(|g| coa_core::upnp::external_ip(g).ok());
        Ok(InternetCheck {
            public_ip: public.map(|a| a.to_string()),
            router_ip: router.map(|a| a.to_string()),
            reachability: coa_core::net::classify(router, public),
            router_found: gw.is_some(),
        })
    })
    .await
}

#[derive(Serialize)]
struct FriendsResult {
    host: String,
    restart_required: bool,
    note: Option<String>,
}

/// Switch how friends connect. `host` is only needed for the internet mode (the public address from the check).
#[tauri::command]
async fn friends_enable(
    state: State<'_, AppState>,
    id: String,
    mode: coa_core::friends::Mode,
    host: Option<String>,
    lan_address_override: Option<String>,
    use_upnp: bool,
) -> std::result::Result<FriendsResult, UiError> {
    use coa_core::friends::{self, Mode};
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        let ports = layout::read_ports(&root);
        let mut note = None;
        let realms = coa_core::realms::state(&root)?;
        let extra = realms.extra_world_ports();
        let host = match mode {
            Mode::Local => "127.0.0.1".to_string(),
            Mode::Lan => coa_core::net::resolve_lan_host(lan_address_override.as_deref(), coa_core::net::lan_ip())?,
            Mode::Direct => host.filter(|h| !h.is_empty()).ok_or_else(|| Error::Invalid("Check your connection first to learn your public address.".into()))?,
            Mode::Private => coa_core::net::tailscale().ip.ok_or_else(|| Error::Invalid("Tailscale is not connected. Install it, sign in, then try again.".into()))?,
        };
        let open = mode != Mode::Local;
        let changed = friends::set_open(&root, &meta, open)?;
        if open && platform::flavor() == Flavor::Repack {
            coa_core::firewall::ensure_rules_with_extra(&ports, &extra)?;
        }
        if mode == Mode::Direct && use_upnp {
            match coa_core::upnp::discover() {
                Some(gw) => {
                    let lan = coa_core::net::lan_ip().ok_or_else(|| Error::Invalid("No local address.".into()))?;
                    coa_core::upnp::add_mapping(&gw, ports.auth, lan, "Auth")?;
                    coa_core::upnp::add_mapping(&gw, ports.world, lan, "World")?;
                    for port in &extra { coa_core::upnp::add_mapping(&gw, *port, lan, "Extra world")?; }
                    note = Some("Your router was asked to forward the game ports.".to_string());
                }
                None => note = Some("Your router does not support automatic setup; forward the two game ports by hand or use the private network.".to_string()),
            }
        }
        let mut settings = friends::load(&meta);
        settings.select_mode(mode, host.clone(), lan_address_override);
        friends::save(&meta, &settings)?;
        let running = coa_core::process::observe(&root, &ports).world.state == coa_core::process::ServiceState::Running;
        if running {
            friends::apply_realm_address(&root, &settings)?;
        }
        Ok(FriendsResult { host, restart_required: changed && running, note })
    })
    .await
}

#[tauri::command]
async fn friends_package(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<String, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        let host = coa_core::friends::load(&meta)
            .host
            .ok_or_else(|| Error::Invalid("Choose how friends connect first.".into()))?;
        let desktop = if platform::flavor() == Flavor::Docker {
            coa_core::diag::desktop_or_temp()
        } else {
            std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join("Desktop")
        };
        let out = if desktop.is_dir() {
            desktop
        } else {
            std::env::temp_dir()
        }
        .join("CoA-Friend-Setup.zip");
        coa_core::friends::make_friend_package(&root, &host, true, &out)?;
        Ok(out.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
async fn run_diagnostics(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::diag::Report, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        Ok(coa_core::diag::run(&root, &meta))
    })
    .await
}

#[tauri::command]
async fn verify_files(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<Vec<coa_core::diag::FileProblem>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        Ok(coa_core::diag::verify_managed(&root, &meta))
    })
    .await
}

/// Writes a redacted zip for bug reports to the Desktop and returns its path.
#[tauri::command]
async fn export_diagnostics(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<String, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, meta) = install_meta(&root)?;
        let report = coa_core::diag::run(&root, &meta);
        let out = coa_core::diag::desktop_or_temp()
            .join(format!("CoA-Diagnostics-{}.zip", coa_core::diag::stamp()));
        coa_core::diag::export_package(
            &root,
            &dir,
            &data_dir().join("logs").join("manager.log"),
            &meta,
            &report,
            &out,
        )?;
        // Show the file in its folder so nobody has to look for it; failing to open the folder is not a failure.
        let _ = tauri_plugin_opener::reveal_item_in_dir(&out);
        Ok(out.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
async fn list_crashes(
    state: State<'_, AppState>,
    id: String,
    limit: Option<usize>,
) -> std::result::Result<Vec<coa_core::crashes::CrashItem>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::crashes::list(&root, limit.unwrap_or(10))).await
}
#[tauri::command]
async fn console_tail(
    state: State<'_, AppState>,
    id: String,
    source: coa_core::console::Source,
    filter: Option<String>,
    lines: Option<usize>,
) -> std::result::Result<Vec<coa_core::console::Line>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        // A Docker server's database writes to its container's output, not to a file.
        if coa_core::docker::is_docker(&root)
            && matches!(source, coa_core::console::Source::Database)
        {
            return coa_core::docker::logs::database_log(
                &root,
                filter.as_deref(),
                lines.unwrap_or(300).min(2000),
            );
        }
        let path = coa_core::console::log_path(
            &root,
            &data_dir().join("logs").join("manager.log"),
            source,
        );
        coa_core::console::tail(&path, filter.as_deref(), lines.unwrap_or(300).min(2000))
    })
    .await
}

#[tauri::command]
fn console_risk(command: String) -> coa_core::console::Risk {
    coa_core::console::risk(&command)
}

/// Send one command to the world server console. Risky commands need `confirmed`.
#[tauri::command]
async fn console_command(
    state: State<'_, AppState>,
    id: String,
    command: String,
    confirmed: bool,
) -> std::result::Result<String, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let c = coa_core::console::check_command(&command)?.to_string();
        if coa_core::console::risk(&c) == coa_core::console::Risk::Dangerous && !confirmed {
            return Err(Error::Invalid(
                "This command can shut things down or change many records. Confirm it first."
                    .into(),
            ));
        }
        tracing::info!(command = %c, "console command");
        Ra::connect(&root)?.run(&c)
    })
    .await
}

#[tauri::command]
async fn analyze_github_module(
    state: State<'_, AppState>,
    url: String,
    realm_id: String,
) -> std::result::Result<coa_core::analyzer::CompatibilityReport, UiError> {
    if realm_id.is_empty() || realm_id.contains("..") || realm_id.contains('/') || realm_id.contains('\\') {
        return Err(UiError::from(coa_core::Error::Invalid(format!(
            "Invalid realm_id '{}': path traversal characters forbidden",
            realm_id
        ))));
    }

    let realm_root = path_of(&state, &realm_id)?;
    let cache_dir = data_dir().join("github_cache");

    blocking(move || {
        coa_core::analyzer::analyze_github_module_for_realm(&url, &realm_root, Some(&cache_dir))
            .map_err(Into::into)
    })
    .await
}

/// Last push of GitHub repositories (`owner/name`), for modules that are not in the public catalogue (CoA's
/// own). Answers are kept for 12 hours in the source cache; a repository GitHub does not answer for gets none.
#[tauri::command]
async fn repo_last_push(repos: Vec<String>) -> std::result::Result<BTreeMap<String, Option<String>>, UiError> {
    let cache_file = data_dir().join("github_cache").join("repo-last-push.json");
    blocking(move || coa_core::catalogue::repo_last_push(&cache_file, &repos)).await
}

/// What the module build cache of a server (and the shared source downloads) take on disk.
#[tauri::command]
async fn build_cache_summary(
    state: State<'_, AppState>,
    id: String,
) -> std::result::Result<coa_core::build_cache::CacheSummary, UiError> {
    let root = path_of(&state, &id)?;
    let sources = data_dir().join("github_cache");
    blocking(move || Ok(coa_core::build_cache::summary(&root, Some(&sources)))).await
}

/// Remove the build cache of one module (`module`), of all modules (no `module`), and optionally the downloaded
/// sources. Refused while a build runs on this server. Returns the bytes freed.
#[tauri::command]
async fn build_cache_clear(
    state: State<'_, AppState>,
    id: String,
    module: Option<String>,
    sources: bool,
) -> std::result::Result<u64, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    let sources_dir = data_dir().join("github_cache");
    blocking(move || {
        let mut freed = 0;
        if module.is_some() || !sources {
            freed += coa_core::build_cache::clear(&root, module.as_deref())?;
        }
        if sources {
            freed += coa_core::build_cache::clear_sources(&sources_dir)?;
        }
        Ok(freed)
    })
    .await
}

#[tauri::command]
async fn get_public_module_catalog(
    force_refresh: Option<bool>,
) -> std::result::Result<coa_core::catalogue::PublicCatalogueResponse, UiError> {
    let cache_dir = data_dir().join("catalogue");
    let force = force_refresh.unwrap_or(false);
    blocking(move || {
        coa_core::catalogue::fetch_or_load_catalogue(&cache_dir, force)
    })
    .await
}

#[tauri::command]
fn preview_toolchain_status(
    state: State<'_, AppState>,
    realm_id: Option<String>,
) -> coa_core::build_engine::ToolchainStatus {
    // The SDK depends on the realm's core: without a realm only the compiler tools are reported.
    let realm_root = realm_id.and_then(|id| path_of(&state, &id).ok());
    coa_core::build_engine::detect_toolchain_for(realm_root.as_deref())
}

/// Runs Microsoft's Build Tools installer with the MSVC toolset this realm's core was built with.
#[tauri::command]
async fn preview_install_msvc(
    state: State<'_, AppState>,
    realm_id: Option<String>,
) -> std::result::Result<coa_core::msvc_setup::MsvcSetupOutcome, UiError> {
    let realm_root = realm_id.and_then(|id| path_of(&state, &id).ok());
    blocking(move || coa_core::msvc_setup::install_build_tools(realm_root.as_deref())).await
}

#[tauri::command]
async fn preview_build_module(
    app: AppHandle,
    state: State<'_, AppState>,
    realm_id: String,
    source_url_or_path: String,
) -> std::result::Result<coa_core::build_engine::BuildResult, UiError> {
    if realm_id.is_empty() || realm_id.contains("..") || realm_id.contains('/') || realm_id.contains('\\') {
        return Err(UiError::from(coa_core::Error::Invalid(format!(
            "Invalid realm_id '{}': path traversal characters forbidden",
            realm_id
        ))));
    }

    let realm_root = path_of(&state, &realm_id)?;
    let cache_dir = data_dir().join("github_cache");
    // The build works in the module's build cache: clearing the cache waits for it.
    let _guard = BusyGuard::acquire(&state, &realm_id)?;

    let app_clone = app.clone();
    let progress_cb: Box<dyn Fn(coa_core::build_engine::BuildProgressEvent) + Send + Sync> =
        Box::new(move |event| {
            let _ = app_clone.emit("preview_build_progress", &event);
        });

    blocking(move || {
        let source_dir = if source_url_or_path.starts_with("http://") || source_url_or_path.starts_with("https://") {
            let report = coa_core::analyzer::analyze_github_module_for_realm(&source_url_or_path, &realm_root, Some(&cache_dir))?;
            let p = PathBuf::from(&report.repository_path);
            if p.is_dir() {
                p
            } else {
                let (owner, repo, _) = coa_core::analyzer::parse_and_validate_github_url(&source_url_or_path)?;
                let repo_cache = cache_dir.join("github").join(&owner).join(&repo);
                let mut found = None;
                if let Ok(entries) = std::fs::read_dir(&repo_cache) {
                    for entry in entries.flatten() {
                        if entry.path().is_dir() && entry.path().join(".coa_tree_digest").is_file() {
                            found = Some(entry.path());
                            break;
                        }
                    }
                }
                found.ok_or_else(|| coa_core::Error::Invalid("Failed to locate cached repository sources after intake".into()))?
            }
        } else {
            let p = PathBuf::from(&source_url_or_path);
            if p.exists() {
                p
            } else if source_url_or_path.len() >= 7 && source_url_or_path.chars().all(|c| c.is_ascii_hexdigit()) {
                // Defensive fallback: search github cache for this digest folder
                let mut found = None;
                if let Ok(entries) = std::fs::read_dir(cache_dir.join("github")) {
                    'outer: for owner_entry in entries.flatten() {
                        if let Ok(repo_entries) = std::fs::read_dir(owner_entry.path()) {
                            for repo_entry in repo_entries.flatten() {
                                let candidate = repo_entry.path().join(&source_url_or_path);
                                if candidate.is_dir() && candidate.join(".coa_tree_digest").is_file() {
                                    found = Some(candidate);
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
                found.ok_or_else(|| coa_core::Error::Invalid(format!("Source path '{}' does not exist", p.display())))?
            } else {
                return Err(coa_core::Error::Invalid(format!("Source path '{}' does not exist", p.display())));
            }
        };

        // Dungeon Clear derives from SQUID's C++ classes, so both are built into the SQUID DLL: building
        // either one builds that DLL, with Dungeon Clear in it when it is installed in this realm. The
        // build cache keeps SQUID's objects, so a Dungeon Clear update recompiles only its own files.
        let (module_id, _, _) = coa_core::build_engine::resolve_module_identity(&source_dir);
        let dungeon_clear_installed = {
            let dir = coa_core::plugins::modules_config_dir(&realm_root);
            dir.join("mod_dungeon_clear.conf").is_file() || dir.join("mod_dungeon_clear.conf.dist").is_file()
        };
        let (primary, bundled) = match module_id.as_str() {
            "dungeon-clear" => {
                let squid = cached_catalog_tree(&cache_dir, "playerbots").ok_or_else(|| {
                    coa_core::Error::Invalid("Dungeon Clear is built into SQUID's Playerbots: install SQUID's Playerbots first.".into())
                })?;
                (squid, vec![source_dir])
            }
            "playerbots" if dungeon_clear_installed => {
                (source_dir, cached_catalog_tree(&cache_dir, "dungeon-clear").into_iter().collect())
            }
            _ => (source_dir, Vec::new()),
        };

        coa_core::build_engine::build_module_with_bundle(&primary, &bundled, &realm_root, None, Some(progress_cb))
            .map_err(Into::into)
    })
    .await
}

/// The newest cached source tree of a catalog module (fetched earlier by the module intake).
fn cached_catalog_tree(cache_dir: &std::path::Path, catalog_id: &str) -> Option<PathBuf> {
    let entry = coa_core::modules::catalog().into_iter().find(|e| e.id == catalog_id)?;
    let (owner, repo, _) = coa_core::analyzer::parse_and_validate_github_url(&entry.repo).ok()?;
    std::fs::read_dir(cache_dir.join("github").join(owner).join(repo)).ok()?
        .flatten()
        .filter(|e| e.path().join(".coa_tree_digest").is_file())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

#[tauri::command]
async fn preview_install_module(
    state: State<'_, AppState>,
    realm_id: String,
    module_id: String,
    version: String,
    apply_sql: bool,
) -> std::result::Result<coa_core::preview::PreviewInstallResult, UiError> {
    if realm_id.is_empty() || realm_id.contains("..") || realm_id.contains('/') || realm_id.contains('\\') {
        return Err(UiError::from(coa_core::Error::Invalid(format!(
            "Invalid realm_id '{}': path traversal characters forbidden",
            realm_id
        ))));
    }

    let realm_root = path_of(&state, &realm_id)?;

    blocking(move || {
        coa_core::preview::install_qualified_module(&realm_root, &module_id, &version, apply_sql, None)
            .map_err(Into::into)
    })
    .await
}

#[tauri::command]
async fn preview_toggle_module(
    state: State<'_, AppState>,
    realm_id: String,
    module_id: String,
    enable: bool,
) -> std::result::Result<String, UiError> {
    let realm_root = path_of(&state, &realm_id)?;
    blocking(move || {
        coa_core::preview::toggle_preview_module(&realm_root, &module_id, enable)
            .map_err(Into::into)
    })
    .await
}

#[tauri::command]
async fn preview_remove_module(
    state: State<'_, AppState>,
    realm_id: String,
    module_id: String,
) -> std::result::Result<String, UiError> {
    let realm_root = path_of(&state, &realm_id)?;
    blocking(move || {
        coa_core::preview::remove_preview_module(&realm_root, &module_id)
            .map_err(Into::into)
    })
    .await
}

#[tauri::command]
async fn preview_list_installed_plugins(
    state: State<'_, AppState>,
    realm_id: String,
) -> std::result::Result<Vec<coa_core::preview::InstalledPreviewPluginView>, UiError> {
    let realm_root = path_of(&state, &realm_id)?;
    blocking(move || {
        coa_core::preview::list_installed_preview_plugins(&realm_root)
            .map_err(Into::into)
    })
    .await
}

#[tauri::command]
async fn preview_export_diagnostics(
    state: State<'_, AppState>,
    realm_id: Option<String>,
) -> std::result::Result<String, UiError> {
    let dir = data_dir();
    let tc = coa_core::build_engine::detect_toolchain_for(
        realm_id.as_deref().and_then(|id| path_of(&state, id).ok()).as_deref(),
    );
    let export_path = dir.join("diagnostics_report.json");

    let mut realms = Vec::new();
    if let Ok(list) = state.registry.list() {
        for (id, p) in list {
            realms.push(serde_json::json!({
                "id": id,
                "path": p.display().to_string(),
            }));
        }
    }

    let report = serde_json::json!({
        "app_title": "CSM Modules Preview",
        "git_branch": "preview/modules",
        "timestamp_utc": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        "toolchain_status": tc,
        "preview_data_dir": dir.display().to_string(),
        "registered_realms": realms,
        "active_realm_id": realm_id,
        "security_note": "No passwords, connection strings, auth tokens or user secrets are included in this report."
    });

    let json_str = serde_json::to_string_pretty(&report)
        .map_err(|e| UiError::from(coa_core::Error::Invalid(e.to_string())))?;
    std::fs::write(&export_path, json_str)
        .map_err(|e| UiError::from(coa_core::Error::Invalid(e.to_string())))?;
    Ok(export_path.display().to_string())
}

#[tauri::command]
async fn modular_copy_preview(path: String) -> std::result::Result<coa_core::modular_copy::CopyPreview, UiError> {
    blocking(move || coa_core::modular_copy::preview(Path::new(&path)).map_err(Into::into)).await
}

#[tauri::command]
async fn preview_check_updates(
    state: State<'_, AppState>,
    realm_id: String,
) -> std::result::Result<coa_core::solver::SolverVerdict, UiError> {
    let realm_root = path_of(&state, &realm_id)?;
    blocking(move || {
        // What the installed worldserver.exe accepts, read from the binary (its .coapl section).
        let installed_host_fp = coa_core::provenance::realm_host_fingerprint(&realm_root);
        let candidate_lock_path = std::env::var("COA_PREVIEW_LOCK_PATH")
            .map(std::path::PathBuf::from)
            .ok()
            .or_else(|| {
                let p1 = realm_root.join(".manager").join("csm-preview-components.json");
                if p1.is_file() { return Some(p1); }
                let p2 = realm_root.join(".manager").join("candidate-lock.json");
                if p2.is_file() { return Some(p2); }
                None
            });
        let candidate_host = if let Some(lock_path) = candidate_lock_path {
            let content = std::fs::read_to_string(&lock_path)
                .map_err(|e| coa_core::Error::Invalid(e.to_string()))?;
            let val: serde_json::Value = serde_json::from_str(&content)
                .map_err(|e| coa_core::Error::Invalid(e.to_string()))?;
            coa_core::solver::HostManifest {
                version: val.get("version").and_then(|v| v.as_str()).unwrap_or("0.7.1-preview").into(),
                commit: val.get("core").and_then(|c| c.get("commit")).and_then(|v| v.as_str()).unwrap_or("unknown").into(),
                abi_fingerprint: val.get("sdk").and_then(|s| s.get("abi_fingerprint")).and_then(|v| v.as_str()).unwrap_or("").into(),
                sdk_contract_version: val.get("sdk").and_then(|s| s.get("version")).and_then(|v| v.as_str()).unwrap_or("0.3.2").into(),
                schema_version: "2026_10_28_00".into(),
            }
        } else {
            coa_core::solver::HostManifest {
                version: "0.7.1-preview".into(),
                commit: "unknown".into(),
                abi_fingerprint: installed_host_fp.clone().unwrap_or_default(),
                sdk_contract_version: "0.3.2".into(),
                schema_version: "2026_10_28_00".into(),
            }
        };

        let installed_host = coa_core::solver::HostManifest {
            version: "0.7.0".into(),
            commit: "installed_commit".into(),
            abi_fingerprint: installed_host_fp.clone().unwrap_or_default(),
            sdk_contract_version: "0.3.2".into(),
            schema_version: "2026_10_28_00".into(),
        };

        let modules_view = coa_core::modules::list(&realm_root);
        let installed_modules: Vec<coa_core::solver::ModuleArtifactInfo> = modules_view
            .into_iter()
            .filter(|m| m.installed)
            .map(|m| coa_core::solver::ModuleArtifactInfo {
                id: m.id.clone(),
                name: m.name,
                version: m.version.unwrap_or_else(|| "1.0.0".into()),
                required_abi_fingerprint: coa_core::plugins::installed_plugin_fingerprint(&realm_root, &m.id),
                required_sdk_contract: Some("0.3.2".into()),
                min_host_version: Some("0.7.0".into()),
                dependencies: vec![],
                conflicts: if m.id == "companions" { vec!["playerbots".into()] } else { vec![] },
                required_schema_version: None,
                has_source: false,
                is_dynamic_dll: m.component_type == coa_core::modules::ComponentType::DynamicDll,
            })
            .collect();

        let graph = coa_core::solver::CompatibilityGraph {
            installed_host: &installed_host,
            candidate_host: &candidate_host,
            installed_modules: &installed_modules,
            available_modules: &[],
            current_schema_version: "2026_10_28_00",
        };

        Ok(coa_core::solver::solve_compatibility(&graph))
    })
    .await
}

#[tauri::command]
fn manager_installing(enabled: bool) {
    QUIT_REQUESTED.store(enabled, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command]
fn close_manager(app: AppHandle) -> std::result::Result<(), UiError> {
    if window_preferences::close_to_tray(&data_dir()).unwrap_or(false) {
        QUIT_REQUESTED.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(window) = app.get_webview_window("main") {
            window.hide().map_err(|e| Error::Invalid(e.to_string()))?;
        }
    } else {
        QUIT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
        app.exit(0);
    }
    Ok(())
}

#[tauri::command]
fn close_to_tray() -> std::result::Result<bool, UiError> {
    Ok(window_preferences::close_to_tray(&data_dir())?)
}

#[tauri::command]
fn set_close_to_tray(enabled: bool) -> std::result::Result<(), UiError> {
    Ok(window_preferences::save(&data_dir(), enabled)?)
}

fn show_main_window(app: &AppHandle) {
    let handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window("main") {
            let result = window
                .unminimize()
                .and_then(|_| window.show())
                .and_then(|_| window.set_focus());
            if let Err(error) = result {
                tracing::error!(%error, "Could not restore the Manager window");
            }
        } else {
            tracing::error!("The Manager main window is missing");
        }
    }) {
        tracing::error!(%error, "Could not schedule Manager window restoration");
    }
}

static QUIT_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn run() {
    let dir = data_dir();
    let _ = coa_core::logging::init(&dir.join("logs").join("manager.log"));
    // Module sources fetched during updates land in the same cache as those the Modules page fetches.
    coa_core::rebuild::set_source_cache(dir.join("github_cache"));
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_main_window(app)
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        // Size, position and maximized state come back after a restart. Visibility is left to the app (tray, start
        // options), so a window last hidden in the tray still opens normally.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            server_logging_preset,
            release_history,
            preview_toolchain_status,
            preview_install_msvc,
            preview_build_module,
            preview_install_module,
            preview_toggle_module,
            preview_remove_module,
            preview_list_installed_plugins,
            preview_export_diagnostics,
            modular_copy_preview,
            preview_check_updates,
            default_install_dir,
            game_data_folder,
            set_game_data_folder,
            install_environment,
            scan_server,
            add_server,
            list_servers,
            forget_server,
            server_status,
            start_server,
            stop_server,
            force_stop_server,
            get_settings,
            save_settings,
            list_presets,
            preview_preset,
            list_config_snapshots,
            restore_config_snapshot,
            list_backups,
            dashboard_status,
            dashboard_install,
            dashboard_start,
            dashboard_open,
            dashboard_stop,
            backup_location,
            set_backup_location,
            create_backup,
            verify_backup,
            delete_backup,
            restore_backup_configs,
            restore_backup_database,
            install_preflight,
            install_requirements,
            report_context,
            list_accounts,
            realmlist_profiles,
            realmlist_save,
            realmlist_delete,
            realmlist_activate,
            modules_list,
            analyze_github_module,
            get_public_module_catalog,
            realm_profiles,
            realm_select,
            realm_simultaneous,
            realm_with_reborn,
            check_database,
            repair_server,
            plugins_repair,
            module_set_enabled,
            module_settings,
            module_save_settings,
            all_settings,
            all_settings_save,
            account_set_password,
            account_set_access,
            account_rename,
            account_delete,
            install_new,
            is_classic_server,
            build_cache_summary,
            build_cache_clear,
            repo_last_push,
            cancel_install,
            create_account,
            check_update,
            pending_update,
            apply_update,
            rollback_update,
            retry_update_validation,
            get_population,
            get_performance,
            companions_stop_spawning,
            companions_take_offline,
            companions_delete_all,
            companions_despawn_some,
            open_link,
            report_targets,
            companion_sizes,
            add_companions,
            remote_connection,
            remote_connect,
            client_info,
            client_graphics,
            client_set_graphics,
            set_client,
            client_realmlist,
            client_install_addon,
            client_status,
            client_download_check,
            client_plan,
            client_sync,
            client_download,
            client_cancel,
            play,
            friends_status,
            friends_check_internet,
            friends_enable,
            friends_package,
            run_diagnostics,
            verify_files,
            export_diagnostics,
            list_crashes,
            console_tail,
            console_risk,
            console_command,
            custom_races_status,
            race_availability,
            race_availability_save,
            custom_races_install,
            custom_races_uninstall,
            custom_races_toggle_client,
            portable_state,
            portable_preflight,
            portable_play,
            portable_local_characters,
            portable_make,
            portable_resolve,
            portable_history,
            portable_add_realm,
            portable_remove_realm,
            portable_diagnostics,
            portable_launch,
            registry_status,
            portable_preflight_remote,
            manager_installing,
            close_manager,
            close_to_tray,
            set_close_to_tray,
            browse_enabled,
            browse_set_enabled,
            browse_list,
            browse_detail,
            module_catalog,
            registry_set_url,
            registry_publish,
            registry_unpublish,
            registry_retry,
            control_cmds::control_status,
            control_cmds::control_set_preferred_username,
            control_cmds::registry_set_access,
            control_cmds::join_realm,
            control_cmds::join_link,
            control_cmds::join_account,
            control_cmds::join_credentials,
            control_cmds::join_forget,
            control_cmds::join_characters,
            control_cmds::join_claim
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                if window_preferences::close_to_tray(&data_dir()).unwrap_or(false) {
                    api.prevent_close();
                    if let Err(error) = window.hide() {
                        tracing::warn!(%error, "Could not hide the Manager window");
                    }
                } else {
                    QUIT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
        })
        .setup(|app| {
            let dir = data_dir();
            // Crash safety: recover any unfinished client realmlist overrides from previous abnormal termination
            let _ = coa_core::client::revert_realmlist_override(&dir);
            let portable = start_portable(&dir);
            let (control, host_control) = control_cmds::start(&dir, portable.clone());
            let registry = Registry::at(dir.join("installs.json"));
            // Development only: an explicitly named test realm is registered into an empty list.
            if registry.list().map(|l| l.is_empty()).unwrap_or(true) {
                if let Ok(path) = std::env::var("COA_TEST_REALM_DIR").map(PathBuf::from) {
                    if path.join("worldserver.exe").is_file() {
                        let _ = registry.register("preview-realm", &path);
                    }
                }
            }
            app.manage(AppState {
                registry,
                busy: Mutex::new(HashSet::new()),
                install_cancel: Mutex::new(None),
                client_cancel: Mutex::new(None),
                portable,
                realm_registry: start_realm_registry(&dir),
                control,
                host_control,
            });

            #[cfg(desktop)]
            {
                use tauri::menu::{Menu, MenuItem};
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

                let show =
                    MenuItem::with_id(app, "show", "Open CoA Server Manager", true, None::<&str>)?;
                let quit = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;
                let menu = Menu::with_items(app, &[&show, &quit])?;

                let mut builder = TrayIconBuilder::new()
                    .tooltip("CoA Server Manager")
                    .menu(&menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id.as_ref() {
                        "show" => {
                            show_main_window(app);
                        }
                        "quit" => {
                            QUIT_REQUESTED.store(true, std::sync::atomic::Ordering::SeqCst);
                            app.exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } = event
                        {
                            let app = tray.app_handle();
                            show_main_window(app);
                        }
                    });

                if let Some(icon) = app.default_window_icon() {
                    builder = builder.icon(icon.clone());
                }

                builder.build(app)?;
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building CoA Server Manager")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { api, .. } => {
                if !QUIT_REQUESTED.load(std::sync::atomic::Ordering::SeqCst) {
                    api.prevent_exit();
                }
            }
            tauri::RunEvent::Exit => {
                // the Host loop stops with the application: what it is doing is finished first, the stores keep every session
                if let Some(rt) = app.state::<AppState>().portable.clone() {
                    rt.shutdown();
                }
                if let Some(rt) = app.state::<AppState>().realm_registry.clone() {
                    rt.shutdown();
                }
                if let Some(rt) = app.state::<AppState>().host_control.clone() {
                    rt.shutdown();
                }
            }
            _ => {}
        });
}

// ---- Public listing (Registry) -----------------------------------------------------------------------------------------------

fn realm_registry(
    state: &State<'_, AppState>,
) -> std::result::Result<Arc<coa_core::realm_registry::RegistryRuntime>, UiError> {
    state
        .realm_registry
        .clone()
        .ok_or_else(|| Error::Invalid("the publishing service did not start".into()).into())
}

#[tauri::command]
async fn portable_preflight_remote(
    state: State<'_, AppState>,
    character: String,
    capabilities: serde_json::Value,
) -> std::result::Result<portable_service::PreflightView, ServiceError> {
    portable_call(&state, move |s| {
        s.preflight_advert(&character, &capabilities)
    })
    .await
}

fn browse_client() -> std::result::Result<coa_core::realm_registry::BrowseClient, UiError> {
    browse_preferences::require_enabled(&data_dir())?;
    let settings = coa_core::realm_registry::settings::load(&data_dir().join("registry"))?;
    let url = std::env::var("COA_REGISTRY_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| settings.effective_url().to_string());
    Ok(coa_core::realm_registry::BrowseClient::new(&url)?)
}

#[tauri::command]
fn browse_enabled() -> std::result::Result<bool, UiError> {
    Ok(browse_preferences::enabled(&data_dir())?)
}

#[tauri::command]
fn browse_set_enabled(enabled: bool) -> std::result::Result<(), UiError> {
    Ok(browse_preferences::save(&data_dir(), enabled)?)
}

fn browse_error(e: coa_core::realm_registry::ClientError) -> UiError {
    Error::Invalid(e.to_string()).into()
}

/// One page of the public realm list (a plain read of the Registry; nothing about the player is sent).
#[tauri::command]
async fn browse_list(
    params: coa_core::realm_registry::BrowseParams,
) -> std::result::Result<coa_registry_proto::RealmPage, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let query = params.to_query()?;
        browse_client()?.list(&query).map_err(browse_error)
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?
}

#[tauri::command]
async fn browse_detail(
    realm_id: String,
) -> std::result::Result<coa_registry_proto::RealmDetail, UiError> {
    tauri::async_runtime::spawn_blocking(move || {
        let id = coa_registry_proto::RealmId::parse(&realm_id)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        browse_client()?.detail(&id).map_err(browse_error)
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?
}

#[derive(serde::Serialize)]
struct ModuleInfo {
    id: String,
    name: String,
    description: std::collections::BTreeMap<String, String>,
    status: String,
    icon: String,
}

/// The Manager's own module catalog: names and descriptions of the modules a realm's advertisement lists by id.
#[tauri::command]
fn module_catalog() -> Vec<ModuleInfo> {
    coa_core::modules::catalog()
        .into_iter()
        .map(|e| ModuleInfo {
            id: e.id,
            name: e.name,
            description: e.description,
            status: e.status,
            icon: e.icon,
        })
        .collect()
}

#[tauri::command]
fn registry_status(state: State<'_, AppState>) -> coa_core::realm_registry::RegistryStatus {
    state
        .realm_registry
        .as_ref()
        .map(|r| r.status())
        .unwrap_or_default()
}

#[tauri::command]
async fn registry_set_url(
    state: State<'_, AppState>,
    url: Option<String>,
) -> std::result::Result<(), UiError> {
    let rt = realm_registry(&state)?;
    tauri::async_runtime::spawn_blocking(move || rt.set_url(url))
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
        .map_err(UiError::from)
}

#[tauri::command]
async fn registry_publish(
    state: State<'_, AppState>,
    local_id: String,
    display_name: String,
    description: String,
    language: String,
    region: Option<String>,
) -> std::result::Result<(), UiError> {
    let rt = realm_registry(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        rt.publish(
            &local_id,
            &display_name,
            &description,
            &language,
            region.as_deref(),
        )
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?
    .map_err(UiError::from)
}

#[tauri::command]
async fn registry_unpublish(
    state: State<'_, AppState>,
    local_id: String,
) -> std::result::Result<(), UiError> {
    let rt = realm_registry(&state)?;
    tauri::async_runtime::spawn_blocking(move || rt.unpublish(&local_id))
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
        .map_err(UiError::from)
}

#[tauri::command]
fn registry_retry(state: State<'_, AppState>, local_id: String) {
    if let Some(r) = &state.realm_registry {
        r.retry(&local_id);
    }
}

fn start_realm_registry(
    dir: &std::path::Path,
) -> Option<Arc<coa_core::realm_registry::RegistryRuntime>> {
    use coa_core::realm_registry::{FileKeyStore, LocalRealmsSource, RegistryRuntime};
    let installs = Registry::at(dir.join("installs.json"));
    let registry_dir = dir.join("registry");
    let source = LocalRealmsSource::with_provisioning(
        dir.join("portable").join("realms"),
        move || installs.list().unwrap_or_default(),
        move |local_id| {
            // a realm creates accounts for joining players unless its owner said it only links the ones that exist
            coa_core::realm_registry::settings::load(&registry_dir)
                .ok()
                .and_then(|s| s.realms.get(local_id).map(|c| !c.existing_only))
                .unwrap_or(true)
        },
    );
    let keys = FileKeyStore::new(dir.join("registry").join("keys"));
    match RegistryRuntime::start(&dir.join("registry"), Arc::new(keys), Arc::new(source)) {
        Ok(rt) => Some(Arc::new(rt)),
        Err(e) => {
            tracing::error!(error = %e, "the publishing service did not start");
            None
        }
    }
}

fn start_portable(dir: &std::path::Path) -> Option<Arc<PortableRuntime>> {
    let registry = Registry::at(dir.join("installs.json"));
    let control = InstallControl {
        registry: Registry::at(dir.join("installs.json")),
    };
    let installs: portable_service::runtime::Installs =
        Box::new(move || registry.list().unwrap_or_default());
    match PortableRuntime::start(&dir.join("portable"), installs, Some(Box::new(control))) {
        Ok(rt) => Some(Arc::new(rt)),
        Err(e) => {
            tracing::error!(error = %e, "the portable play service did not start");
            None
        }
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::unix_data_home;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn os(s: &str) -> Option<OsString> {
        Some(s.into())
    }

    #[test]
    fn data_home_prefers_an_absolute_xdg_folder() {
        assert_eq!(
            unix_data_home(os("/data/xdg"), os("/home/u")),
            PathBuf::from("/data/xdg")
        );
    }

    #[test]
    fn data_home_ignores_a_relative_xdg_folder_and_falls_back_to_home() {
        assert_eq!(
            unix_data_home(os("relative/dir"), os("/home/u")),
            PathBuf::from("/home/u/.local/share")
        );
        assert_eq!(
            unix_data_home(None, os("/home/u")),
            PathBuf::from("/home/u/.local/share")
        );
    }

    #[test]
    fn data_home_without_any_variable_uses_the_temp_folder() {
        assert_eq!(unix_data_home(None, None), std::env::temp_dir());
    }
}
