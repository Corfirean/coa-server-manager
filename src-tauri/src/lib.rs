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
    /// Cancel handle of the game-client check or download currently running, if any.
    client_cancel: Mutex<Option<Cancel>>,
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

/// Where the Manager keeps its own state (server list, logs). `%LOCALAPPDATA%` on Windows, the XDG data folder elsewhere.
fn data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
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
                if coa_core::realms::guard_module(&r, "companions").is_err() { return Ok(()); }
                let pending = meta_dir(&r)?.join("companions.pending.json");
                if let Ok(v) = coa_core::fsx::read_json::<serde_json::Value>(&pending) {
                    let _ = std::fs::remove_file(&pending);
                    let n = v["count"].as_u64().unwrap_or(0) as u32;
                    if n > 0 {
                        let made = coa_core::db::Db::from_repack(&r, coa_core::db::Account::Admin).and_then(|db| coa_core::companions::ensure_templates(&db));
                        if let Err(e) = made.and_then(|_| Ra::connect(&r)).and_then(|mut ra| ra.spawn_bots(n)) {
                            tracing::warn!("pending companions were not created: {e}");
                        }
                    }
                }
                Ok(())
            })
            .await;
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
fn install_preflight(state: State<'_, AppState>, dest: String, needed: Option<u64>) -> Preflight {
    // `needed` is the real size from the signed package list when the screen already has it; the real size is checked
    // again at install time either way.
    install::preflight(std::path::Path::new(&dest), needed.unwrap_or(6 * 1024 * 1024 * 1024), &state.registry)
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
async fn install_requirements(package: Option<String>) -> std::result::Result<InstallRequirements, UiError> {
    blocking(move || {
        let (m, _) = coa_core::pkgsource::fetch_manifest(&package_source(package), coa_core::signing::EMBEDDED_PUBLIC_KEY)?;
        let archive = m.archive.ok_or_else(|| Error::InvalidManifest("no archive".into()))?;
        Ok(InstallRequirements { download_bytes: archive.parts.iter().map(|p| p.size).sum(), unpacked_bytes: archive.unpacked_size, version: m.version })
    })
    .await
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

#[tauri::command]
async fn realmlist_profiles(state: State<'_, AppState>, id: String) -> std::result::Result<coa_core::realmlist::View, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        Ok(coa_core::realmlist::view(&dir, linked_client(&root)?.as_deref()))
    })
    .await
}

#[tauri::command]
async fn realmlist_save(state: State<'_, AppState>, id: String, profile_id: Option<String>, name: String, data: String) -> std::result::Result<coa_core::realmlist::Profile, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::realmlist::save_profile(&dir, linked_client(&root)?.as_deref(), profile_id.as_deref(), &name, &data)
    })
    .await
}

#[tauri::command]
async fn realmlist_delete(state: State<'_, AppState>, id: String, profile_id: String) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::realmlist::delete_profile(&dir, linked_client(&root)?.as_deref(), &profile_id)
    })
    .await
}

/// Write the chosen realmlist into the linked game client.
#[tauri::command]
async fn realmlist_activate(state: State<'_, AppState>, id: String, profile_id: String) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        let client = linked_client(&root)?.ok_or_else(|| Error::Invalid("No game client is set up for this server yet.".into()))?;
        let changed = coa_core::realmlist::activate(&dir, &client, &profile_id)?;
        tracing::info!(profile = %profile_id, files = changed.len(), "realmlist switched");
        Ok(changed)
    })
    .await
}

#[tauri::command]
async fn modules_list(state: State<'_, AppState>, id: String) -> std::result::Result<Vec<coa_core::modules::ModuleView>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || Ok(coa_core::modules::list(&root))).await
}

#[tauri::command]
async fn realm_profiles(state: State<'_, AppState>, id: String) -> std::result::Result<coa_core::realms::View, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::realms::view(&root)).await
}

#[tauri::command]
async fn realm_select(state: State<'_, AppState>, id: String, mode: coa_core::realms::Mode, restart: bool) -> std::result::Result<coa_core::realms::View, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    let result = blocking(move || {
        if restart {
            let out = driver::run(&root, Verb::StopAll)?;
            if !out.ok { return Err(Error::Invalid(out.output)); }
        }
        let view = coa_core::realms::select(&root, mode)?;
        let (dir, meta) = install_meta(&root)?;
        if let Some(client) = meta.client_path {
            coa_core::client::sync_realm(std::path::Path::new(&client), &dir, view.active)?;
        }
        if restart {
            let out = driver::run(&root, Verb::StartAll)?;
            if !out.ok { return Err(Error::Invalid(format!("Realm selected, but startup failed: {}", out.output))); }
        }
        Ok(view)
    }).await;
    result
}

#[tauri::command]
async fn realm_simultaneous(state: State<'_, AppState>, id: String, enabled: bool) -> std::result::Result<coa_core::realms::View, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || coa_core::multiworld::set_enabled(&root, enabled)).await
}

#[tauri::command]
async fn check_database(state: State<'_, AppState>, id: String) -> std::result::Result<Vec<coa_core::repair::DatabaseCheck>, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || coa_core::repair::check(&root, &meta_dir(&root)?)).await
}

#[tauri::command]
async fn repair_server(app: AppHandle, state: State<'_, AppState>, id: String) -> std::result::Result<coa_core::repair::Report, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let dir = meta_dir(&root)?;
        coa_core::repair::run(&root, &dir, &package_source(None), &update_source(None), coa_core::signing::EMBEDDED_PUBLIC_KEY, &|step,percent| {
            let _ = app.emit("repair-progress", serde_json::json!({ "id": id, "step": step, "percent": percent }));
        })
    }).await
}

#[tauri::command]
async fn module_set_enabled(state: State<'_, AppState>, id: String, module: String, enabled: bool) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::modules::set_enabled(&root, &dir, &module, enabled)
    })
    .await
}

#[tauri::command]
async fn module_settings(state: State<'_, AppState>, id: String, module: String) -> std::result::Result<Vec<coa_core::modules::Setting>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::modules::settings(&root, &module)).await
}

#[tauri::command]
async fn module_save_settings(state: State<'_, AppState>, id: String, module: String, changes: BTreeMap<String, String>) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::modules::save_settings(&root, &dir, &module, &changes)
    })
    .await
}

#[tauri::command]
async fn all_settings(state: State<'_, AppState>, id: String) -> std::result::Result<Vec<coa_core::allsettings::Item>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::allsettings::list(&root)).await
}

#[tauri::command]
async fn all_settings_save(state: State<'_, AppState>, id: String, changes: BTreeMap<String, String>) -> std::result::Result<Vec<String>, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        coa_core::allsettings::save(&root, &dir, &changes)
    })
    .await
}

#[tauri::command]
async fn list_accounts(state: State<'_, AppState>, id: String) -> std::result::Result<Vec<coa_core::accounts::AccountInfo>, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || coa_core::accounts::list(&root)).await
}

#[tauri::command]
async fn account_set_password(state: State<'_, AppState>, id: String, name: String, password: String) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        Ra::connect(&root)?.set_account_password(&name, &password)?;
        tracing::info!(%name, "account password changed");
        Ok(())
    })
    .await
}

#[tauri::command]
async fn account_set_access(state: State<'_, AppState>, id: String, name: String, level: u8) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        Ra::connect(&root)?.set_account_access(&name, level)?;
        tracing::info!(%name, level, "account access level changed");
        Ok(())
    })
    .await
}

#[tauri::command]
async fn account_rename(state: State<'_, AppState>, id: String, name: String, new_name: String, password: String) -> std::result::Result<(), UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let mut ra = Ra::connect(&root)?;
        coa_core::accounts::rename(&root, &mut ra, &name, &new_name, &password)
    })
    .await
}

#[tauri::command]
async fn account_delete(state: State<'_, AppState>, id: String, name: String) -> std::result::Result<(), UiError> {
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
    let src = update_source(source);
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let (dir, _) = install_meta(&root)?;
        // Files cannot be replaced while the server runs: stop it first (gracefully), like the Stop button.
        let observed = coa_core::process::observe(&root, &layout::read_ports(&root));
        if observed.world.state != coa_core::process::ServiceState::Stopped || observed.auth.state != coa_core::process::ServiceState::Stopped || coa_core::multiworld::is_running(&root) {
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
async fn retry_update_validation(state: State<'_, AppState>, id: String, txn: String) -> std::result::Result<update::Txn, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let dir = meta_dir(&root)?;
        let pending = update::load(&dir, &txn)?;
        let source = coa_core::pkgsource::Source::Url(format!("https://github.com/Corfirean/coa-server-build/releases/download/server-{}/", pending.to_version));
        let env = update::RepackEnv { root: &root, meta_dir: &dir };
        update::retry_validation(&root, &dir, &txn, &source, coa_core::signing::EMBEDDED_PUBLIC_KEY, &env)
    }).await
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
struct CompanionAction {
    /// Bots affected (cancelled from the queue, or logged out).
    count: u32,
}

#[tauri::command]
async fn companions_stop_spawning(state: State<'_, AppState>, id: String) -> std::result::Result<CompanionAction, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || Ok(CompanionAction { count: Ra::connect(&root)?.cancel_spawning()? })).await
}

#[tauri::command]
async fn companions_take_offline(state: State<'_, AppState>, id: String) -> std::result::Result<CompanionAction, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let mut ra = Ra::connect(&root)?;
        // Anything still waiting to be created must not come back right after.
        let _ = ra.cancel_spawning();
        Ok(CompanionAction { count: ra.despawn_all()? })
    })
    .await
}

/// Log `count` randomly chosen online companions out to lower the load. Uses the per-bot command that every server
/// build has, so it works on older builds too.
#[tauri::command]
async fn companions_despawn_some(state: State<'_, AppState>, id: String, count: u32) -> std::result::Result<CompanionAction, UiError> {
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
async fn companions_delete_all(app: AppHandle, state: State<'_, AppState>, id: String) -> std::result::Result<CompanionAction, UiError> {
    let root = path_of(&state, &id)?;
    let _guard = BusyGuard::acquire(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        backup::create(&root, &meta, Kind::Quick, Trigger::BeforeDangerousChange, Some("before deleting all companions".into()), &|step| {
            let _ = app.emit("backup-progress", step);
        })?;
        let before = coa_core::population::query(&root).map(|p| p.bots_total).unwrap_or(0);
        let mut ra = Ra::connect(&root)?;
        let _ = ra.cancel_spawning();
        ra.purge_all()?;
        Ok(CompanionAction { count: before })
    })
    .await
}

#[tauri::command]
async fn get_performance(state: State<'_, AppState>, id: String) -> std::result::Result<Option<coa_core::ra::Performance>, UiError> {
    let root = path_of(&state, &id)?;
    Ok(tauri::async_runtime::spawn_blocking(move || {
        let o = coa_core::process::observe(&root, &layout::read_ports(&root));
        if o.world.state != coa_core::process::ServiceState::Running {
            return None;
        }
        Ra::connect(&root).and_then(|mut r| r.performance()).ok().flatten()
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
        // Every companion, old and new, should come back at the next start, so the start-up limit only ever grows here.
        let existing_max = config::load(&root, Scope::Bots)
            .ok()
            .and_then(|v| v.settings.iter().find(|s| s.meta.key == "CoaBots.AutoLogin.MaxCount").and_then(|s| s.value.as_u64()))
            .unwrap_or(0) as u32;
        let already = coa_core::population::query(&root).map(|p| p.bots_total).unwrap_or(0);
        changes.insert("CoaBots.AutoLogin.MaxCount".to_string(), Value::from(existing_max.max(already + count).min(5000)));
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
                coa_core::companions::ensure_templates(&coa_core::db::Db::from_repack(&root, coa_core::db::Account::Admin)?)?;
                coa_core::companions::offline_create(&root, &meta.join("logs").join("companions-offline.log"), count)?;
                Ok(())
            })();
            if started_db {
                let _ = driver::run(&root, Verb::StopAll);
            }
            made?;
            tracing::info!(count, "companions created offline");
            return Ok(CompanionsResult { spawned: None, created: Some(count), baseline: None });
        }
        let mut baseline = None;
        let spawned = if o.world.state == coa_core::process::ServiceState::Running {
            // Older servers have no template characters to copy bots from: add them first.
            coa_core::companions::ensure_templates(&coa_core::db::Db::from_repack(&root, coa_core::db::Account::Admin)?)?;
            baseline = coa_core::population::query(&root).ok().map(|p| p.bots_total);
            Some(Ra::connect(&root)?.spawn_bots(count)?)
        } else {
            // The server is stopped: creating bots needs it running, so remember the request and do it after the next start.
            coa_core::fsx::atomic_write_json(&meta.join("companions.pending.json"), &serde_json::json!({ "count": count }))?;
            None
        };
        tracing::info!(count, spawned = spawned.is_some(), "companions requested");
        Ok(CompanionsResult { spawned, created: None, baseline })
    })
    .await
}

const REMOTE_CLIENT_ID: &str = "@remote-client";

fn remote_dir() -> PathBuf { data_dir().join("remote-client") }

#[tauri::command]
fn remote_connection() -> std::result::Result<coa_core::remote_client::Profile, UiError> {
    Ok(coa_core::remote_client::load(&remote_dir())?)
}

#[tauri::command]
fn remote_connect(state: State<'_, AppState>, host: String) -> std::result::Result<coa_core::remote_client::Profile, UiError> {
    let job = state.client_cancel.lock().unwrap();
    if job.is_some() { return Err(Error::Invalid("Wait for the client operation to finish.".into()).into()); }
    let dir = remote_dir();
    let mut profile = coa_core::remote_client::load(&dir)?;
    let host = host.trim();
    if host.is_empty() { return Err(Error::Invalid("Enter the host's IP address or hostname.".into()).into()); }
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
        return Ok(ClientContext { dir, path: profile.client_path.map(PathBuf::from), source: None, host: profile.host, root: None });
    }
    let root = path_of(state, id)?;
    let (dir, meta) = install_meta(&root)?;
    let source = coa_core::client::addon_source(&root);
    Ok(ClientContext { dir, path: meta.client_path.map(PathBuf::from), source, host: "127.0.0.1".into(), root: Some(root) })
}

impl ClientContext {
    fn linked(&self) -> Option<PathBuf> {
        self.path.clone().filter(|p| coa_core::client::detect(p, None).is_some())
    }
    fn require(&self) -> Result<PathBuf> {
        self.linked().ok_or_else(|| Error::Invalid("Set up a game client first.".into()))
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
    let client = meta.client_path.clone().map(PathBuf::from).ok_or_else(|| Error::Invalid("No game client is set up for this server yet.".into()))?;
    Ok((dir, meta, Some(client)))
}

#[tauri::command]
fn client_info(state: State<'_, AppState>, id: String) -> std::result::Result<Option<coa_core::client::ClientInfo>, UiError> {
    let ctx = client_context(&state, &id)?;
    Ok(ctx.path.and_then(|p| coa_core::client::detect(&p, ctx.source.as_deref())))
}

#[tauri::command]
fn set_client(state: State<'_, AppState>, id: String, path: String) -> std::result::Result<coa_core::client::ClientInfo, UiError> {
    let job = state.client_cancel.lock().unwrap();
    if job.is_some() { return Err(Error::Invalid("Wait for the client operation to finish.".into()).into()); }
    let ctx = client_context(&state, &id)?;
    let info = coa_core::client::detect(std::path::Path::new(&path), ctx.source.as_deref())
        .ok_or_else(|| Error::Invalid("This folder does not look like a game client (it needs Data and the game executable).".into()))?;
    ctx.link(&info.path)?;
    Ok(coa_core::client::detect(std::path::Path::new(&info.path), ctx.source.as_deref()).unwrap_or(info))
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
    Ok(meta.client_path.map(PathBuf::from).filter(|p| coa_core::client::detect(p, None).is_some()))
}

/// Cheap: one small manifest request and the local state file; no game file is read.
#[tauri::command]
async fn client_status(state: State<'_, AppState>, id: String) -> std::result::Result<ClientStatus, UiError> {
    let ctx = client_context(&state, &id)?;
    blocking(move || {
        let client = ctx.linked();
        let latest = coa_core::clientdl::fetch_latest().ok();
        let local = client.as_deref().map(coa_core::clientdl::local);
        let managed = local.as_ref().map(|l| l.managed).unwrap_or(false);
        let installed = local.and_then(|l| l.version);
        let update_available = managed && latest.as_ref().map(|l| installed.as_deref() != Some(l.version.as_str())).unwrap_or(false);
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
async fn client_download_check(parent: String) -> std::result::Result<ClientDownloadCheck, UiError> {
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
async fn client_plan(app: AppHandle, state: State<'_, AppState>, id: String) -> std::result::Result<coa_core::clientdl::Plan, UiError> {
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
async fn client_sync(app: AppHandle, state: State<'_, AppState>, id: String, keep_modified: bool) -> std::result::Result<(), UiError> {
    let ctx = client_context(&state, &id)?;
    let cancel = begin_client_job(&state)?;
    let result = blocking(move || {
        let client = ctx.require()?;
        run_client_sync(&app, &client, keep_modified, &cancel)?;
        if ctx.root.is_none() && !ctx.host.is_empty() { coa_core::client::set_realmlist(&client, &ctx.dir, &ctx.host)?; }
        Ok(())
    })
    .await;
    end_client_job(&state);
    result
}

fn run_client_sync(app: &AppHandle, client: &std::path::Path, keep_modified: bool, cancel: &Cancel) -> Result<()> {
    let emit = |s: coa_core::clientdl::Step| {
        let _ = app.emit("client-progress", s);
    };
    let manifest = coa_core::clientdl::fetch_latest()?;
    let current = coa_core::clientdl::load_state(client).unwrap_or_default();
    let plan = coa_core::clientdl::plan(client, &manifest, &current, cancel, &emit)?;
    coa_core::download::check_url(coa_core::clientdl::OBJECTS_URL)?;
    let transport = coa_core::download::HttpTransport::new()?;
    coa_core::clientdl::apply(
        &coa_core::clientdl::Apply { client, manifest: &manifest, plan: &plan, keep_modified, transport: &transport, objects_url: coa_core::clientdl::OBJECTS_URL, cancel },
        &emit,
    )?;
    tracing::info!(version = %manifest.version, downloaded = plan.items.len(), "client brought up to date");
    Ok(())
}

/// Download the client into `<parent>/CoA Client`, then link it to a host or player profile.
#[tauri::command]
async fn client_download(app: AppHandle, state: State<'_, AppState>, id: String, parent: String) -> std::result::Result<coa_core::client::ClientInfo, UiError> {
    let ctx = client_context(&state, &id)?;
    let cancel = begin_client_job(&state)?;
    let result = blocking(move || {
        let parent = PathBuf::from(&parent);
        if !parent.is_dir() {
            return Err(Error::Invalid("Choose an existing folder to put the game client in.".into()));
        }
        let dest = parent.join(CLIENT_FOLDER);
        let resuming = coa_core::clientdl::load_state(&dest).is_some();
        let non_empty = std::fs::read_dir(&dest).map(|mut r| r.next().is_some()).unwrap_or(false);
        if non_empty && !resuming {
            return Err(Error::Invalid(format!("{} already exists and is not an unfinished download. Choose another folder.", dest.display())));
        }
        std::fs::create_dir_all(&dest)?;
        run_client_sync(&app, &dest, false, &cancel)?;
        let info = coa_core::client::detect(&dest, ctx.source.as_deref()).ok_or_else(|| Error::Invalid("The downloaded client looks incomplete.".into()))?;
        ctx.link(&info.path)?;
        if !ctx.host.is_empty() { coa_core::client::set_realmlist(&dest, &ctx.dir, &ctx.host)?; }
        if let Some(src) = &ctx.source { coa_core::client::install_addon(&dest, &ctx.dir, src)?; }
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
    let mut slot = state.client_cancel.lock().map_err(|_| Error::Invalid("state poisoned".into()))?;
    if slot.is_some() {
        return Err(Error::Invalid("The game client is already being checked or downloaded.".into()).into());
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
async fn play(state: State<'_, AppState>, id: String) -> std::result::Result<DriverOutcome, UiError> {
    if id == REMOTE_CLIENT_ID {
        let ctx = client_context(&state, &id)?;
        let client = ctx.require()?;
        return blocking(move || {
            if ctx.host.is_empty() { return Err(Error::Invalid("Enter the host's IP address or hostname first.".into())); }
            if coa_core::client::is_running(&client) { return Err(Error::Invalid("The game is already running.".into())); }
            coa_core::client::set_realmlist(&client, &ctx.dir, &ctx.host)?;
            coa_core::client::launch(&client)?;
            Ok(DriverOutcome { ok: true, exit_code: None, code: None, human: None, output: String::new() })
        }).await;
    }
    let root = path_of(&state, &id)?;
    let (dir, _, client) = client_of(&root)?;
    let client = client.unwrap();
    if coa_core::client::is_running(&client) {
        return Err(Error::Invalid("Close the game client before pressing Play so its selected realm can be updated.".into()).into());
    }
    blocking(move || {
        let mode = coa_core::realms::state(&root)?.active;
        if !coa_core::client::sync_realm(&client, &dir, mode)? {
            return Err(Error::Invalid("Close the game client before pressing Play so its selected realm can be updated.".into()));
        }
        coa_core::client::launch(&client)?;
        Ok(DriverOutcome { ok: true, exit_code: None, code: None, human: None, output: String::new() })
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
    firewall: coa_core::firewall::Status,
    tailscale: coa_core::net::Tailscale,
    server_running: bool,
    auth_port: u16,
    world_port: u16,
    secondary_world_port: Option<u16>,
}

/// Open one of a few known help pages in the default browser. Anything else is refused, so a page can never ask the
/// Manager to launch an arbitrary address.
#[tauri::command]
fn open_link(url: String) -> std::result::Result<(), UiError> {
    const ALLOWED: &[&str] = &["https://tailscale.com/", "https://login.tailscale.com/", "https://portforward.com/", "https://github.com/Corfirean/"];
    // A prefilled "new issue" page of one of the places a report can go (the Manager, Companions, SQUID Playerbots): the
    // address must be exactly that page, and its query may carry `&` between the (percent-encoded) title and text.
    let issue = coa_core::report::is_new_issue_url(&url);
    let chars_ok = url.chars().all(|c| c.is_ascii_alphanumeric() || "/:._-?=#%".contains(c) || (issue && c == '&'));
    // the GitHub page of a module that is in the bundled catalog (some are not ours)
    let catalog = coa_core::modules::catalog().iter().any(|e| e.repo == url);
    if !(issue || catalog || ALLOWED.iter().any(|p| url.starts_with(p))) || !chars_ok || url.len() > 12_000 {
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

/// "Windows 11 (build 26200)" from `ver`; empty if it cannot be read.
fn windows_version() -> String {
    let mut cmd = std::process::Command::new("cmd.exe");
    cmd.args(["/C", "ver"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let build: Option<u32> = out.split("Version").nth(1).and_then(|v| v.trim().trim_end_matches(']').split('.').nth(2)).and_then(|b| b.trim().parse().ok());
    match build {
        Some(b) if b >= 22000 => format!("Windows 11 (build {b})"),
        Some(b) => format!("Windows 10 (build {b})"),
        None => String::new(),
    }
}

/// What the problem report fills in on its own. Nothing here identifies the person or the machine.
#[tauri::command]
async fn report_context(state: State<'_, AppState>, id: String) -> std::result::Result<ReportContext, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let (_, meta) = install_meta(&root)?;
        let bots = coa_core::report::bots(&root);
        let companions = [meta.bots.version.clone(), meta.bots.commit.as_ref().map(|c| c.chars().take(8).collect())].into_iter().flatten().collect::<Vec<String>>().join(" · ");
        Ok(ReportContext {
            manager_version: coa_core::MANAGER_VERSION.to_string(),
            windows: windows_version(),
            install_kind: if meta.kind == coa_core::registry::InstallKind::New { "new".into() } else { "imported".into() },
            server_version: meta.core.version.clone(),
            suggested_target: bots.suggested.to_string(),
            bots_version: if bots.suggested == "squid" { bots.squid_version } else if bots.suggested == "companions" && !companions.is_empty() { Some(companions) } else { None },
        })
    })
    .await
}

#[tauri::command]
async fn friends_status(state: State<'_, AppState>, id: String) -> std::result::Result<FriendsStatus, UiError> {
    let root = path_of(&state, &id)?;
    blocking(move || {
        let meta = meta_dir(&root)?;
        let ports = layout::read_ports(&root);
        let automatic = coa_core::net::lan_ip().filter(|ip| coa_core::net::is_lan_address(*ip));
        Ok(FriendsStatus {
            settings: coa_core::friends::load(&meta),
            lan_ip: automatic.map(|a| a.to_string()),
            lan_addresses: coa_core::net::lan_addresses(automatic).unwrap_or_default(),
            exposure: coa_core::net::exposure(&ports),
            servers_open: coa_core::friends::bind_is_open(&root),
            firewall: coa_core::firewall::status(),
            tailscale: coa_core::net::tailscale(),
            server_running: coa_core::process::observe(&root, &ports).world.state == coa_core::process::ServiceState::Running,
            auth_port: ports.auth,
            world_port: ports.world,
            secondary_world_port: coa_core::realms::state(&root)?.secondary_world_port.filter(|_| coa_core::realms::state(&root).is_ok_and(|s| s.simultaneous)),
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
        let secondary = realms.secondary_world_port.filter(|_| realms.simultaneous);
        let host = match mode {
            Mode::Local => "127.0.0.1".to_string(),
            Mode::Lan => coa_core::net::resolve_lan_host(lan_address_override.as_deref(), coa_core::net::lan_ip())?,
            Mode::Direct => host.filter(|h| !h.is_empty()).ok_or_else(|| Error::Invalid("Check your connection first to learn your public address.".into()))?,
            Mode::Private => coa_core::net::tailscale().ip.ok_or_else(|| Error::Invalid("Tailscale is not connected. Install it, sign in, then try again.".into()))?,
        };
        let open = mode != Mode::Local;
        let changed = friends::set_open(&root, &meta, open)?;
        if open {
            coa_core::firewall::ensure_rules_with_secondary(&ports, secondary)?;
        }
        if mode == Mode::Direct && use_upnp {
            match coa_core::upnp::discover() {
                Some(gw) => {
                    let lan = coa_core::net::lan_ip().ok_or_else(|| Error::Invalid("No local address.".into()))?;
                    coa_core::upnp::add_mapping(&gw, ports.auth, lan, "Auth")?;
                    coa_core::upnp::add_mapping(&gw, ports.world, lan, "World")?;
                    if let Some(port) = secondary { coa_core::upnp::add_mapping(&gw, port, lan, "Second world")?; }
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
        // Show the file in its folder so nobody has to look for it; failing to open the folder is not a failure.
        let _ = tauri_plugin_opener::reveal_item_in_dir(&out);
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
        .manage(AppState { registry: Registry::at(dir.join("installs.json")), busy: Mutex::new(HashSet::new()), install_cancel: Mutex::new(None), client_cancel: Mutex::new(None) })
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
            install_requirements,
            report_context,
            list_accounts,
            realmlist_profiles,
            realmlist_save,
            realmlist_delete,
            realmlist_activate,
            modules_list,
            realm_profiles,
            realm_select,
            realm_simultaneous,
            check_database,
            repair_server,
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
            console_tail,
            console_risk,
            console_command
        ])
        .run(tauri::generate_context!())
        .expect("error while running CoA Server Manager");
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
        assert_eq!(unix_data_home(os("/data/xdg"), os("/home/u")), PathBuf::from("/data/xdg"));
    }

    #[test]
    fn data_home_ignores_a_relative_xdg_folder_and_falls_back_to_home() {
        assert_eq!(unix_data_home(os("relative/dir"), os("/home/u")), PathBuf::from("/home/u/.local/share"));
        assert_eq!(unix_data_home(None, os("/home/u")), PathBuf::from("/home/u/.local/share"));
    }

    #[test]
    fn data_home_without_any_variable_uses_the_temp_folder() {
        assert_eq!(unix_data_home(None, None), std::env::temp_dir());
    }
}
