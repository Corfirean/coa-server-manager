//! The Linux (Docker) side of the install commands. `lib.rs` asks `coa_core::platform::flavor()` once per command and
//! calls this module for a Docker install; the code for a repack install stays where it was and is not changed by what
//! happens here.

use std::path::{Path, PathBuf};

use coa_core::docker::{self, install as core_install, SystemDocker};
use coa_core::download::Cancel;
use coa_core::install::{Preflight, Source};
use coa_core::registry::Registry;
use coa_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use super::{blocking, data_dir, summary, AppState, InstallRequirements, ServerSummary, UiError};

/// What the install screen needs to know about this computer before it asks anything.
#[derive(Serialize)]
pub struct Environment {
    pub flavor: coa_core::platform::Flavor,
    pub default_dir: String,
    /// Why Docker cannot be used (not installed, not running, no permission), in Docker's own words; none when it works.
    pub docker_problem: Option<String>,
}

pub fn default_dir() -> String {
    default_dir_in(std::env::var("HOME").ok().as_deref())
}

fn default_dir_in(home: Option<&str>) -> String {
    match home.map(|h| h.trim_end_matches('/')).filter(|h| !h.is_empty()) {
        Some(h) => format!("{h}/CoaServer"),
        None => "/srv/coa".into(),
    }
}

pub fn docker_problem() -> Option<String> {
    docker::check_docker(&SystemDocker).err().map(|e| e.to_string())
}

pub fn environment() -> Environment {
    Environment { flavor: coa_core::platform::flavor(), default_dir: default_dir(), docker_problem: docker_problem() }
}

pub fn preflight(dest: &str, data: Option<String>, needed: Option<u64>, registry: &Registry) -> Preflight {
    // The size comes from the signed package list when the screen already has it; it is checked again at install time.
    core_install::preflight(Path::new(dest), Path::new(data.as_deref().unwrap_or("")), needed.unwrap_or(1024 * 1024 * 1024), registry)
}

/// The Linux package has no default address yet: it is published only after the maintainer has signed it.
pub fn package_source(custom: Option<String>) -> Result<Source> {
    let pick = custom.filter(|s| !s.trim().is_empty()).or_else(|| std::env::var("COA_PACKAGE_SOURCE").ok().filter(|s| !s.trim().is_empty()));
    match pick {
        Some(p) if p.to_ascii_lowercase().starts_with("http") => Ok(Source::Url(p)),
        Some(p) => Ok(Source::Dir(PathBuf::from(p))),
        None => Err(Error::Invalid("Choose the Linux server package (a folder or an address): none is published yet.".into())),
    }
}

/// The key a Linux package must be signed with. A debug build (`tauri dev`) may name another one in `COA_DEV_TRUSTED_KEY`,
/// to try the installer on a package signed with a throwaway key; a release build always uses the embedded key.
pub fn trusted_key() -> String {
    #[cfg(debug_assertions)]
    if let Ok(k) = std::env::var("COA_DEV_TRUSTED_KEY") {
        if !k.trim().is_empty() {
            return k.trim().to_string();
        }
    }
    coa_core::signing::EMBEDDED_PUBLIC_KEY.to_string()
}

pub async fn requirements(package: Option<String>) -> std::result::Result<InstallRequirements, UiError> {
    blocking(move || {
        let (m, _) = coa_core::pkgsource::fetch_manifest(&package_source(package)?, &trusted_key())?;
        let archive = m.archive.ok_or_else(|| Error::InvalidManifest("no archive".into()))?;
        Ok(InstallRequirements { download_bytes: archive.parts.iter().map(|p| p.size).sum(), unpacked_bytes: archive.unpacked_size, version: m.version })
    })
    .await
}

pub async fn install(app: AppHandle, state: State<'_, AppState>, dest: String, package: Option<String>, data: Option<String>) -> std::result::Result<ServerSummary, UiError> {
    let source = package_source(package)?;
    let data_dir_path = PathBuf::from(data.filter(|d| !d.trim().is_empty()).ok_or_else(|| Error::Invalid("Choose the folder that holds the game data.".into()))?);
    let cancel = Cancel::default();
    *state.install_cancel.lock().map_err(|_| Error::Invalid("state poisoned".into()))? = Some(cancel.clone());
    let registry = Registry::at(data_dir().join("installs.json"));
    let dest_path = PathBuf::from(dest);
    let key = trusted_key();
    let result = tauri::async_runtime::spawn_blocking(move || {
        core_install::install(
            &core_install::Params { source, dest: dest_path, data_dir: data_dir_path, trusted_key: &key, registry: &registry, cancel },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_folder_is_in_the_home_folder() {
        assert_eq!(default_dir_in(Some("/home/ana")), "/home/ana/CoaServer");
        assert_eq!(default_dir_in(Some("/home/ana/")), "/home/ana/CoaServer");
        assert_eq!(default_dir_in(None), "/srv/coa");
        assert_eq!(default_dir_in(Some("")), "/srv/coa");
    }

    #[test]
    fn a_linux_package_must_be_named_there_is_no_default() {
        std::env::remove_var("COA_PACKAGE_SOURCE");
        assert!(package_source(None).is_err());
        assert!(package_source(Some("   ".into())).is_err());
        assert!(matches!(package_source(Some("/tmp/pkg".into())).unwrap(), Source::Dir(_)));
        assert!(matches!(package_source(Some("https://example.org/pkg".into())).unwrap(), Source::Url(_)));
    }
}
