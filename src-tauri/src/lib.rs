//! Thin Tauri command layer. Every mutating command takes an installation *id* and resolves the path from the
//! registry; the frontend can never ask the backend to touch an arbitrary path (scan is read-only).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use coa_core::driver::{self, DriverOutcome, Verb};
use coa_core::error::UiError;
use coa_core::layout::{self, Classification, ScanReport};
use coa_core::process::{self, Observed};
use coa_core::registry::{metadata_dir_for, InstallKind, InstallMeta, MetaDir, Registry};
use coa_core::{Error, Result};
use serde::Serialize;
use tauri::State;

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
            stop_server
        ])
        .run(tauri::generate_context!())
        .expect("error while running CoA Server Manager");
}
