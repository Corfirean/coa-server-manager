//! Thin Tauri command layer. Every mutating command takes an installation *id* and resolves the path from the
//! registry; the frontend can never ask the backend to touch an arbitrary path (scan is read-only).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use coa_core::backup::{self, Kind, RecoveryPoint, Trigger, VerifyReport};
use coa_core::config::{self, Scope, SettingsView};
use coa_core::download::Cancel;
use coa_core::driver::{self, DriverOutcome, Verb};
use coa_core::install::{self, Preflight, Source};
use coa_core::ra::Ra;
use coa_core::update::{self, Resolution};
use coa_core::error::UiError;
use coa_core::layout::{self, Classification, ScanReport};
use coa_core::process::{self, Observed};
use coa_core::registry::{metadata_dir_for, InstallKind, InstallMeta, MetaDir, Registry};
use coa_core::{Error, Result};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, State};

struct AppState {
    registry: Registry,
    /// Installation ids with a start/stop currently running (one action at a time per server).
    busy: Mutex<HashSet<String>>,
    /// Cancel handle of the installation currently running, if any.
    install_cancel: Mutex<Option<Cancel>>,
}

/// Where official server packages are published (created by the release pipeline, Phase 6).
/// Where signed update packages are published; override with COA_UPDATE_SOURCE (URL or local package folder).
const DEFAULT_UPDATE_URL: &str = "https://github.com/Corfirean/coa-server-build/releases/download/stable";

fn update_source(custom: Option<String>) -> Source {
    let pick = custom.filter(|s| !s.trim().is_empty()).or_else(|| std::env::var("COA_UPDATE_SOURCE").ok());
    match pick {
        Some(p) if p.to_ascii_lowercase().starts_with("http") => Source::Url(p),
        Some(p) => Source::Dir(PathBuf::from(p)),
        None => Source::Url(DEFAULT_UPDATE_URL.into()),
    }
}

const DEFAULT_PACKAGE_URL: &str = "https://github.com/Corfirean/coa-server-build/releases/download/base";

fn package_source(custom: Option<String>) -> Source {
    let pick = custom.filter(|s| !s.trim().is_empty()).or_else(|| std::env::var("COA_PACKAGE_SOURCE").ok());
    match pick {
        Some(p) if p.to_ascii_lowercase().starts_with("http") => Source::Url(p),
        Some(p) => Source::Dir(PathBuf::from(p)),
        None => Source::Url(DEFAULT_PACKAGE_URL.into()),
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

fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("CoAServerManager")
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
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Server".into());
    ServerSummary { id, name, path: path.to_string_lossy().into_owned() }
}

#[tauri::command]
fn default_install_dir() -> String {
    "C:\\Games\\CoA Server".into()
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
async fn add_server(state: State<'_, AppState>, path: String) -> std::result::Result<ServerSummary, UiError> {
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
    meta.core.commit = report.release.as_ref().and_then(|r| r.main_revision.clone());
    meta.core.version = report.banner_revision.clone();
    meta.database.port = Some(report.ports.mysql);
    meta.database.schemas = report.database_schemas.clone();
    for (rel, exe) in [("Core/worldserver.exe", &report.worldserver), ("Core/authserver.exe", &report.authserver)] {
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
    Ok(state.registry.list()?.into_iter().map(|(id, p)| summary(id, p)).collect())
}

#[tauri::command]
fn forget_server(state: State<'_, AppState>, id: String) -> std::result::Result<(), UiError> {
    // Removes only the registry entry; no files are deleted.
    Ok(state.registry.unregister(&id)?)
}

#[tauri::command]
async fn server_status(state: State<'_, AppState>, id: String) -> std::result::Result<StatusView, UiError> {
    let root = path_of(&state, &id)?;
    let busy = state.busy.lock().map(|b| b.contains(&id)).unwrap_or(false);
    let path_exists = root.is_dir();
    let observed = tauri::async_runtime::spawn_blocking(move || {
        let ports = layout::read_ports(&root);
        process::observe(&root, &ports)
    })
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(StatusView { observed, busy, path_exists })
}

async fn run_verb(state: &AppState, id: String, verb: Verb) -> std::result::Result<DriverOutcome, UiError> {
    let root = path_of(state, &id)?;
    if !state.busy.lock().map_err(|_| Error::Invalid("state poisoned".into()))?.insert(id.clone()) {
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
    result.map_err(|e| Error::Invalid(e.to_string()))?.map_err(Into::into)
}

#[tauri::command]
async fn start_server(state: State<'_, AppState>, id: String) -> std::result::Result<DriverOutcome, UiError> {
    let out = run_verb(&state, id.clone(), Verb::StartAll).await?;
    if out.ok {
        if let Ok(root) = path_of(&state, &id) {
            let _ = tauri::async_runtime::spawn_blocking(move || meta_dir(&root).and_then(|m| coa_core::friends::reapply(&root, &m))).await;
        }
    }
    Ok(out)
}

#[tauri::command]
async fn stop_server(state: State<'_, AppState>, id: String) -> std::result::Result<DriverOutcome, UiError> {
    run_verb(&state, id, Verb::StopAll).await
}


#[derive(Serialize)]
struct PresetInfo {
    id: String,
    title: String,
    description: String,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> std::result::Result<T, UiError> {
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
        Err(Error::Invalid("This server has not been added to the Manager yet.".into()))
    }
}

#[tauri::command]
async fn get_settings(state: State<'_, AppState>, id: String, scope: Scope) -> std::result::Result<SettingsView, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || config::load(&root, scope)).await
}

#[tauri::command]
async fn save_settings(
    state: State<'_, AppState>,
    id: String,
    scope: Scope,
    changes: BTreeMap<String, Value>,
) -> std::result::Result<config::SaveReport, UiError> {
    let root = path_of(&state, &id)?;
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
        .map(|p| PresetInfo { id: p.id.clone(), title: p.title.clone(), description: p.description.clone() })
        .collect()
}

#[tauri::command]
async fn preview_preset(state: State<'_, AppState>, id: String, scope: Scope, preset: String) -> std::result::Result<config::PresetPreview, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || if preset == "defaults" { config::preview_defaults(&root, scope) } else { config::preview_preset(&root, scope, &preset) }).await
}

#[tauri::command]
fn list_config_snapshots(state: State<'_, AppState>, id: String) -> std::result::Result<Vec<config::SnapshotInfo>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(config::list_snapshots(&meta_dir(&root)?))
}

#[tauri::command]
async fn restore_config_snapshot(state: State<'_, AppState>, id: String, snapshot: String) -> std::result::Result<(), UiError> {
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
        let mut b = state.busy.lock().map_err(|_| Error::Invalid("state poisoned".into()))?;
        if !b.insert(id.to_string()) {
            return Err(Error::Invalid("Another action is still in progress for this server.".into()));
        }
        Ok(Self { state, id: id.to_string() })
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
fn list_backups(state: State<'_, AppState>, id: String) -> std::result::Result<Vec<RecoveryPoint>, UiError> {
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
async fn verify_backup(state: State<'_, AppState>, id: String, backup_id: String) -> std::result::Result<VerifyReport, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || backup::verify(&meta_dir(&root)?, &backup_id)).await
}

#[tauri::command]
fn delete_backup(state: State<'_, AppState>, id: String, backup_id: String) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    Ok(backup::delete(&meta_dir(&root)?, &backup_id)?)
}

#[tauri::command]
async fn restore_backup_configs(state: State<'_, AppState>, id: String, backup_id: String) -> std::result::Result<RecoveryPoint, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || backup::restore_configs(&root, &meta_dir(&root)?, &backup_id)).await
}

#[tauri::command]
async fn restore_backup_database(state: State<'_, AppState>, id: String, backup_id: String, database: String) -> std::result::Result<backup::DbRestore, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || backup::restore_database(&root, &meta_dir(&root)?, &backup_id, &database)).await
}

#[tauri::command]
fn install_preflight(state: State<'_, AppState>, dest: String) -> Preflight {
    // The real size is checked again once the signed manifest is known; assume a typical install here.
    install::preflight(std::path::Path::new(&dest), 6 * 1024 * 1024 * 1024, &state.registry)
}

#[tauri::command]
async fn install_new(app: AppHandle, state: State<'_, AppState>, dest: String, package: Option<String>) -> std::result::Result<ServerSummary, UiError> {
    let cancel = Cancel::default();
    *state.install_cancel.lock().map_err(|_| Error::Invalid("state poisoned".into()))? = Some(cancel.clone());
    let source = package_source(package);
    let registry = Registry::at(data_dir().join("installs.json"));
    let dest_path = PathBuf::from(dest);
    let result = tauri::async_runtime::spawn_blocking(move || {
        install::install_base(
            &install::Params { source, dest: dest_path, trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY, registry: &registry, cancel },
            &|step| {
                let _ = app.emit("install-progress", step);
            },
        )
    })
    .await;
    if let Ok(mut c) = state.install_cancel.lock() {
        *c = None;
    }
    let done = result.map_err(|e| Error::Invalid(e.to_string()))??;
    Ok(summary(done.id, PathBuf::from(done.path)))
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
async fn create_account(state: State<'_, AppState>, id: String, username: String, password: String, administrator: bool) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        coa_core::ra::validate_account(&username, &password)?;
        let mut ra = Ra::connect(&root)?;
        ra.create_account(&username, &password)?;
        if administrator {
            ra.make_administrator(&username)?;
        }
        tracing::info!(%username, administrator, "account created");
        Ok(())
    })
    .await
}

fn install_meta(root: &std::path::Path) -> Result<(PathBuf, InstallMeta)> {
    let dir = meta_dir(root)?;
    let (_, meta) = MetaDir::open(&dir)?;
    Ok((dir, meta))
}

#[tauri::command]
async fn check_update(state: State<'_, AppState>, id: String, source: Option<String>) -> std::result::Result<update::Preview, UiError> {
    let root = path_of(&state, &id)?;
    let src = update_source(source);
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        update::preview(&root, &meta, &src, coa_core::signing::EMBEDDED_PUBLIC_KEY, &Default::default())
    })
    .await
}

#[tauri::command]
fn pending_update(state: State<'_, AppState>, id: String) -> std::result::Result<Option<update::Txn>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(update::unfinished(&meta_dir(&root)?))
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
    let src = update_source(source);
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        // Files cannot be replaced while the server runs: stop it first (gracefully), like the Stop button.
        let observed = coa_core::process::observe(&root, &layout::read_ports(&root));
        if observed.world.state != coa_core::process::ServiceState::Stopped || observed.auth.state != coa_core::process::ServiceState::Stopped {
            let out = driver::run(&root, Verb::StopAll)?;
            if !out.ok {
                return Err(Error::Invalid("The server could not be stopped, so the update was not started.".into()));
            }
        }
        let env = update::RepackEnv { root: &root, meta_dir: &dir };
        update::apply(
            &update::Params { root: &root, meta_dir: &dir, source: src, trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY, cancel: Cancel::default(), resolutions, env: &env, fail_after_ops: None },
            &|step, percent| {
                let _ = app.emit("update-progress", serde_json::json!({ "step": step, "percent": percent }));
            },
        )
    })
    .await
}

#[tauri::command]
async fn rollback_update(state: State<'_, AppState>, id: String, txn: String) -> std::result::Result<update::Txn, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let dir = meta_dir(&root)?;
        let env = update::RepackEnv { root: &root, meta_dir: &dir };
        update::rollback(&root, &dir, &txn, &env)
    })
    .await
}

#[tauri::command]
async fn get_population(state: State<'_, AppState>, id: String) -> std::result::Result<Option<coa_core::population::Population>, UiError> {
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
}

/// Turn on automatic bot login for `count` bots and, if the server is running, ask it to create them.
#[tauri::command]
async fn add_companions(state: State<'_, AppState>, id: String, count: u32) -> std::result::Result<CompanionsResult, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        if !(1..=2000).contains(&count) {
            return Err(Error::Invalid("Choose between 1 and 2000 companions.".into()));
        }
        let meta = meta_dir(&root)?;
        let mut changes = BTreeMap::new();
        changes.insert("CoaBots.AutoLoginOnStartup".to_string(), Value::Bool(true));
        changes.insert("CoaBots.AutoLogin.MaxCount".to_string(), Value::from(count));
        config::save(&root, &meta, Scope::Bots, &changes)?;
        let o = coa_core::process::observe(&root, &layout::read_ports(&root));
        let spawned = if o.world.state == coa_core::process::ServiceState::Running {
            Some(Ra::connect(&root)?.spawn_bots(count)?)
        } else {
            None
        };
        tracing::info!(count, spawned = spawned.is_some(), "companions requested");
        Ok(CompanionsResult { spawned })
    })
    .await
}

fn client_of(root: &std::path::Path) -> Result<(PathBuf, InstallMeta, Option<PathBuf>)> {
    let (dir, meta) = install_meta(root)?;
    let client = meta.client_path.clone().map(PathBuf::from).ok_or_else(|| Error::Invalid("No game client is set up for this server yet.".into()))?;
    Ok((dir, meta, Some(client)))
}

#[tauri::command]
fn client_info(state: State<'_, AppState>, id: String) -> std::result::Result<Option<coa_core::client::ClientInfo>, UiError> {
    let root = path_of(&state, &id)?;
    let (_, meta) = install_meta(&root)?;
    let source = coa_core::client::addon_source(&root);
    Ok(meta.client_path.and_then(|p| coa_core::client::detect(std::path::Path::new(&p), source.as_deref())))
}

#[tauri::command]
fn set_client(state: State<'_, AppState>, id: String, path: String) -> std::result::Result<coa_core::client::ClientInfo, UiError> {
    let root = path_of(&state, &id)?;
    let (dir, mut meta) = install_meta(&root)?;
    let source = coa_core::client::addon_source(&root);
    let info = coa_core::client::detect(std::path::Path::new(&path), source.as_deref())
        .ok_or_else(|| Error::Invalid("This folder does not look like a game client (it needs Data and the game executable).".into()))?;
    meta.client_path = Some(info.path.clone());
    coa_core::fsx::atomic_write_json(&dir.join("install.json"), &meta).map_err(UiError::from)?;
    Ok(info)
}

#[tauri::command]
fn client_realmlist(state: State<'_, AppState>, id: String, host: String) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    let (dir, _, client) = client_of(&root)?;
    Ok(coa_core::client::set_realmlist(&client.unwrap(), &dir, &host)?)
}

#[tauri::command]
fn client_install_addon(state: State<'_, AppState>, id: String) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    let (dir, _, client) = client_of(&root)?;
    let source = coa_core::client::addon_source(&root).ok_or_else(|| Error::Invalid("This server package does not include the companion addon.".into()))?;
    Ok(coa_core::client::install_addon(&client.unwrap(), &dir, &source)?)
}

/// Start the server if needed, wait until it is ready, then launch the game client.
#[tauri::command]
async fn play(state: State<'_, AppState>, id: String) -> std::result::Result<DriverOutcome, UiError> {
    let root = path_of(&state, &id)?;
    let (_, _, client) = client_of(&root)?;
    let client = client.unwrap();
    let ready = {
        let o = coa_core::process::observe(&root, &layout::read_ports(&root));
        [&o.mysql, &o.auth, &o.world].iter().all(|s| s.state == coa_core::process::ServiceState::Running)
    };
    if !ready {
        let out = run_verb(&state, id, Verb::StartAll).await?;
        if !out.ok {
            return Ok(out);
        }
    }
    blocking(move || {
        coa_core::client::launch(&client)?;
        Ok(DriverOutcome { ok: true, exit_code: None, code: None, human: None, output: String::new() })
    })
    .await
}

#[derive(Serialize)]
struct FriendsStatus {
    settings: coa_core::friends::Settings,
    lan_ip: Option<String>,
    exposure: Vec<coa_core::net::Exposure>,
    /// The configuration lets other computers reach the login and world servers.
    servers_open: bool,
    firewall: coa_core::firewall::Status,
    tailscale: coa_core::net::Tailscale,
    server_running: bool,
}

#[tauri::command]
async fn friends_status(state: State<'_, AppState>, id: String) -> std::result::Result<FriendsStatus, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        let ports = layout::read_ports(&root);
        Ok(FriendsStatus {
            settings: coa_core::friends::load(&meta),
            lan_ip: coa_core::net::lan_ip().map(|a| a.to_string()),
            exposure: coa_core::net::exposure(&ports),
            servers_open: coa_core::friends::bind_is_open(&root),
            firewall: coa_core::firewall::status(),
            tailscale: coa_core::net::tailscale(),
            server_running: coa_core::process::observe(&root, &ports).world.state == coa_core::process::ServiceState::Running,
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
        let router = gw.as_ref().and_then(|g| coa_core::upnp::external_ip(g).ok());
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
    use_upnp: bool,
) -> std::result::Result<FriendsResult, UiError> {
    use coa_core::friends::{self, Mode, Settings};
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        let ports = layout::read_ports(&root);
        let mut note = None;
        let host = match mode {
            Mode::Local => "127.0.0.1".to_string(),
            Mode::Lan => coa_core::net::lan_ip().ok_or_else(|| Error::Invalid("This computer has no network address.".into()))?.to_string(),
            Mode::Direct => host.filter(|h| !h.is_empty()).ok_or_else(|| Error::Invalid("Check your connection first to learn your public address.".into()))?,
            Mode::Private => coa_core::net::tailscale().ip.ok_or_else(|| Error::Invalid("Tailscale is not connected. Install it, sign in, then try again.".into()))?,
        };
        let open = mode != Mode::Local;
        let changed = friends::set_open(&root, &meta, open)?;
        if open {
            coa_core::firewall::ensure_rules(&ports)?;
        }
        if mode == Mode::Direct && use_upnp {
            match coa_core::upnp::discover() {
                Some(gw) => {
                    let lan = coa_core::net::lan_ip().ok_or_else(|| Error::Invalid("No local address.".into()))?;
                    coa_core::upnp::add_mapping(&gw, ports.auth, lan, "Auth")?;
                    coa_core::upnp::add_mapping(&gw, ports.world, lan, "World")?;
                    note = Some("Your router was asked to forward the game ports.".to_string());
                }
                None => note = Some("Your router does not support automatic setup; forward the two game ports by hand or use the private network.".to_string()),
            }
        }
        friends::save(&meta, &Settings { mode, host: Some(host.clone()) })?;
        let running = coa_core::process::observe(&root, &ports).world.state == coa_core::process::ServiceState::Running;
        if running {
            friends::apply_realm_address(&root, &host)?;
        }
        Ok(FriendsResult { host, restart_required: changed && running, note })
    })
    .await
}

#[tauri::command]
async fn friends_package(state: State<'_, AppState>, id: String) -> std::result::Result<String, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        let host = coa_core::friends::load(&meta).host.ok_or_else(|| Error::Invalid("Choose how friends connect first.".into()))?;
        let desktop = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Desktop");
        let out = if desktop.is_dir() { desktop } else { std::env::temp_dir() }.join("CoA-Friend-Setup.zip");
        coa_core::friends::make_friend_package(&root, &host, true, &out)?;
        Ok(out.to_string_lossy().into_owned())
    })
    .await
}

#[tauri::command]
async fn run_diagnostics(state: State<'_, AppState>, id: String) -> std::result::Result<coa_core::diag::Report, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        Ok(coa_core::diag::run(&root, &meta))
    })
    .await
}

#[tauri::command]
async fn verify_files(state: State<'_, AppState>, id: String) -> std::result::Result<Vec<coa_core::diag::FileProblem>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        Ok(coa_core::diag::verify_managed(&root, &meta))
    })
    .await
}

/// Writes a redacted zip for bug reports to the Desktop and returns its path.
#[tauri::command]
async fn export_diagnostics(state: State<'_, AppState>, id: String) -> std::result::Result<String, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, meta) = install_meta(&root)?;
        let report = coa_core::diag::run(&root, &meta);
        let out = coa_core::diag::desktop_or_temp().join(format!("CoA-Diagnostics-{}.zip", coa_core::diag::stamp()));
        coa_core::diag::export_package(&root, &dir, &data_dir().join("logs").join("manager.log"), &meta, &report, &out)?;
        Ok(out.to_string_lossy().into_owned())
    })
    .await
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
        let path = coa_core::console::log_path(&root, &data_dir().join("logs").join("manager.log"), source);
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
async fn console_command(state: State<'_, AppState>, id: String, command: String, confirmed: bool) -> std::result::Result<String, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let c = coa_core::console::check_command(&command)?.to_string();
        if coa_core::console::risk(&c) == coa_core::console::Risk::Dangerous && !confirmed {
            return Err(Error::Invalid("This command can shut things down or change many records. Confirm it first.".into()));
        }
        tracing::info!(command = %c, "console command");
        Ra::connect(&root)?.run(&c)
    })
    .await
}

pub fn run() {
    let dir = data_dir();
    let _ = coa_core::logging::init(&dir.join("logs").join("manager.log"));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState { registry: Registry::at(dir.join("installs.json")), busy: Mutex::new(HashSet::new()), install_cancel: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![
            default_install_dir,
            scan_server,
            add_server,
            list_servers,
            forget_server,
            server_status,
            start_server,
            stop_server,
            get_settings,
            save_settings,
            list_presets,
            preview_preset,
            list_config_snapshots,
            restore_config_snapshot,
            list_backups,
            create_backup,
            verify_backup,
            delete_backup,
            restore_backup_configs,
            restore_backup_database,
            install_preflight,
            install_new,
            cancel_install,
            create_account,
            check_update,
            pending_update,
            apply_update,
            rollback_update,
            get_population,
            companion_sizes,
            add_companions,
            client_info,
            set_client,
            client_realmlist,
            client_install_addon,
            play,
            friends_status,
            friends_check_internet,
            friends_enable,
            friends_package,
            run_diagnostics,
            verify_files,
            export_diagnostics,
            console_tail,
            console_risk,
            console_command
        ])
        .run(tauri::generate_context!())
        .expect("error while running CoA Server Manager");
}
