//! Thin Tauri command layer. Every mutating command takes an installation *id* and resolves the path from the
//! registry; the frontend can never ask the backend to touch an arbitrary path (scan is read-only).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use coa_core::backup::{self, Kind, RecoveryPoint, Trigger, VerifyReport};
use coa_core::config::{self, Scope, SettingsView};
use coa_core::driver::{self, DriverOutcome, Verb};
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
    run_verb(&state, id, Verb::StartAll).await
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

pub fn run() {
    let dir = data_dir();
    let _ = coa_core::logging::init(&dir.join("logs").join("manager.log"));
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { registry: Registry::at(dir.join("installs.json")), busy: Mutex::new(HashSet::new()) })
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
            restore_backup_database
        ])
        .run(tauri::generate_context!())
        .expect("error while running CoA Server Manager");
}
