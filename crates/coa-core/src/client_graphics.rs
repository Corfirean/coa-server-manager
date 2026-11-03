use crate::{fsx, Error, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Default, Serialize, Deserialize)]
pub struct Graphics {
    pub dxvk_disabled: bool,
    pub enhancements_disabled: bool,
}

fn preferences(client: &Path) -> Result<Option<Graphics>> {
    match fs::read(client.join(".coa-manager/graphics.json")) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn status(client: &Path) -> Result<Graphics> {
    let saved = preferences(client)?.unwrap_or_default();
    let actual = |name: &str, remembered: bool| -> Result<bool> {
        let active = client.join(name).is_file();
        let inactive = client.join(format!("{name}.DISABLED")).is_file();
        if active && inactive {
            return Err(Error::Invalid(
                "Both enabled and disabled renderer files exist.".into(),
            ));
        }
        Ok(inactive || (!active && remembered))
    };
    Ok(Graphics {
        dxvk_disabled: actual("d3d9.dll", saved.dxvk_disabled)?,
        enhancements_disabled: actual("dxgi.dll", saved.enhancements_disabled)?,
    })
}

pub fn disabled(client: &Path, path: &str) -> Result<bool> {
    let name = path.to_ascii_lowercase();
    if !matches!(name.as_str(), "d3d9.dll" | "dxgi.dll") {
        return Ok(false);
    }
    let state = status(client)?;
    Ok(if name == "d3d9.dll" {
        state.dxvk_disabled
    } else {
        state.enhancements_disabled
    })
}

pub fn set(client: &Path, name: &str, disabled: bool) -> Result<Graphics> {
    if !matches!(name, "d3d9.dll" | "dxgi.dll") {
        return Err(Error::Invalid("Unknown graphics setting.".into()));
    }
    if crate::client::is_running(client) {
        return Err(Error::Invalid(
            "Close the game before changing graphics settings.".into(),
        ));
    }
    let active = fsx::safe_join(client, name)?;
    let inactive = fsx::safe_join(client, &format!("{name}.DISABLED"))?;
    if active.exists() && inactive.exists() {
        return Err(Error::Invalid("Both enabled and disabled renderer files exist. Resolve the conflict before changing graphics settings.".into()));
    }
    let mut state = status(client)?;
    if name == "d3d9.dll" {
        state.dxvk_disabled = disabled;
    } else {
        state.enhancements_disabled = disabled;
    }
    let (source, target) = if disabled {
        (&active, &inactive)
    } else {
        (&inactive, &active)
    };
    // The no-clobber link also preserves an existing destination if another app changes the setting.
    if source.is_file() {
        if let Err(error) = fs::hard_link(source, target) {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)?;
            let copied = std::fs::File::open(source)
                .and_then(|mut input| std::io::copy(&mut input, &mut output))
                .and_then(|_| output.sync_all());
            drop(output);
            if let Err(error) = copied {
                let _ = fs::remove_file(target);
                return Err(error.into());
            }
        }
        if let Err(error) = fs::remove_file(source) {
            let _ = fs::remove_file(target);
            return Err(error.into());
        }
    }
    fsx::atomic_write_json(&client.join(".coa-manager/graphics.json"), &state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn toggles_preserve_bytes_and_refuse_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("d3d9.dll"), b"custom renderer").unwrap();
        assert!(set(root, "d3d9.dll", true).unwrap().dxvk_disabled);
        assert!(!root.join("d3d9.dll").exists());
        assert_eq!(
            fs::read(root.join("d3d9.dll.DISABLED")).unwrap(),
            b"custom renderer"
        );
        assert!(!set(root, "d3d9.dll", false).unwrap().dxvk_disabled);
        fs::write(root.join("d3d9.dll.DISABLED"), b"other").unwrap();
        assert!(set(root, "d3d9.dll", false).is_err());
        assert_eq!(fs::read(root.join("d3d9.dll")).unwrap(), b"custom renderer");
        assert_eq!(fs::read(root.join("d3d9.dll.DISABLED")).unwrap(), b"other");
    }

    #[test]
    fn remembers_missing_wrapper_and_imports_launcher_marker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(set(root, "d3d9.dll", true).unwrap().dxvk_disabled);
        assert!(disabled(root, "d3d9.dll").unwrap());
        fs::write(root.join("dxgi.dll.DISABLED"), b"launcher disabled").unwrap();
        assert!(disabled(root, "dxgi.dll").unwrap());
        assert!(!disabled(root, "Data/d3d9.dll").unwrap());
    }
}
