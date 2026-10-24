//! Prepare the core side of the owner's portable-character opt-in.
use std::{fs, path::Path};

use super::{parser::ConfFile, take_snapshot, targets, Scope};
use crate::{fsx, Error, Result};

/// Configuration is changed only while the world is stopped. Disabling admission is
/// handled by the control service and must not disable an active session's core.
pub fn enable(root: &Path) -> Result<()> {
    let meta = crate::registry::metadata_dir_for(root)?;
    let _lock = crate::update::operation_lock_for(&meta, "configuring portable characters", false)?;
    let module = root.join("Core/configs/modules/coa.conf");
    let module_source = if module.is_file() {
        module.clone()
    } else {
        root.join("Core/configs/modules/coa.conf.dist")
    };
    let bytes = fs::read(&module_source).map_err(|_| Error::Invalid(
        "Update this server before enabling portable characters; its portable configuration is missing.".into()
    ))?;
    let mut conf = ConfFile::parse_bytes(&bytes)?;
    if conf.get("PortableSession.Enable").is_none() {
        return Err(Error::Invalid("Update this server before enabling portable characters; its core configuration has no portable session support.".into()));
    }
    let mut writes = Vec::new();
    if conf.get("PortableSession.Enable").map(str::trim) != Some("1")
        || conf.get("PortableImport.JobDir").map(str::trim) != Some("\"PortableImport\"")
        || !module.is_file()
    {
        conf.set(
            "PortableSession.Enable",
            "1",
            &["Enabled by the realm owner's portable-character opt-in"],
        );
        conf.set(
            "PortableImport.JobDir",
            "\"PortableImport\"",
            &["Shared working directory for the core and Manager"],
        );
        writes.push((
            module.clone(),
            module.is_file().then(|| bytes.clone()),
            conf.to_text(),
        ));
    }
    for path in targets(root, Scope::Server)?.writes {
        let bytes = fs::read(&path)?;
        let mut conf = ConfFile::parse_bytes(&bytes)?;
        if conf.get("CharacterDatabase.WorkerThreads").map(str::trim) != Some("1") {
            conf.set(
                "CharacterDatabase.WorkerThreads",
                "1",
                &["Portable sessions require ordered character database transactions"],
            );
            writes.push((path, Some(bytes), conf.to_text()));
        }
    }
    if writes.is_empty() {
        fs::create_dir_all(root.join("Core/PortableImport"))?;
        return Ok(());
    }
    let observed = crate::process::observe(root, &crate::layout::read_ports(root));
    if observed.world.state != crate::process::ServiceState::Stopped
        || observed.world.conflict.is_some()
        || crate::multiworld::is_running(root)
    {
        return Err(Error::Invalid("Stop this server, then enable portable characters and save again. The Manager must configure the core before its next start.".into()));
    }
    let originals = writes
        .iter()
        .filter_map(|(p, old, _)| old.clone().map(|bytes| (p.clone(), bytes)))
        .collect::<Vec<_>>();
    take_snapshot(
        &meta,
        Scope::Server,
        "before enabling portable characters",
        &originals,
    )?;
    for (index, (path, _, text)) in writes.iter().enumerate() {
        if let Err(error) = fsx::atomic_write(path, text.as_bytes()) {
            for (path, old, _) in writes.iter().take(index) {
                if let Some(bytes) = old {
                    let _ = fsx::atomic_write(path, bytes);
                } else {
                    let _ = fs::remove_file(path);
                }
            }
            return Err(error);
        }
    }
    fs::create_dir_all(root.join("Core/PortableImport"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn portable_opt_in_updates_the_effective_module_and_survives_launcher_rendering() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("realm");
        fs::create_dir_all(root.join("Core/configs/modules")).unwrap();
        fs::create_dir_all(root.join("Settings")).unwrap();
        fs::write(
            root.join("Settings/repack.json"),
            r#"{"worldPort":49143,"authPort":49144,"mysqlPort":49145,"raPort":49146}"#,
        )
        .unwrap();
        fs::write(
            root.join("Core/configs/modules/coa.conf"),
            "# Owner setting\nCoA.Enable = 1\nPortableSession.Enable = 0\n",
        )
        .unwrap();
        for name in [
            "Settings/worldserver.conf.template",
            "Core/configs/worldserver.conf",
        ] {
            fs::write(
                root.join(name),
                "CharacterDatabase.WorkerThreads = 3\nMaxPlayerLevel = 80\n",
            )
            .unwrap();
        }
        enable(&root).unwrap();
        assert!(root.join("Core/PortableImport").is_dir());
        let module = fs::read_to_string(root.join("Core/configs/modules/coa.conf")).unwrap();
        assert!(
            module.contains("# Owner setting") && module.contains("PortableSession.Enable = 1")
        );
        for name in [
            "Settings/worldserver.conf.template",
            "Core/configs/worldserver.conf",
        ] {
            let text = fs::read_to_string(root.join(name)).unwrap();
            assert!(
                text.contains("CharacterDatabase.WorkerThreads = 1")
                    && text.contains("MaxPlayerLevel = 80")
            );
        }
        let snapshots =
            super::super::list_snapshots(&crate::registry::metadata_dir_for(&root).unwrap());
        assert_eq!(snapshots.len(), 1);
        enable(&root).unwrap();
        assert_eq!(
            super::super::list_snapshots(&crate::registry::metadata_dir_for(&root).unwrap()).len(),
            1
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        fs::write(
            root.join("Settings/repack.json"),
            format!(
                r#"{{"worldPort":{}}}"#,
                listener.local_addr().unwrap().port()
            ),
        )
        .unwrap();
        let disabled = "PortableSession.Enable = 0\n";
        fs::write(root.join("Core/configs/modules/coa.conf"), disabled).unwrap();
        assert!(enable(&root)
            .unwrap_err()
            .to_string()
            .contains("Stop this server"));
        assert_eq!(
            fs::read_to_string(root.join("Core/configs/modules/coa.conf")).unwrap(),
            disabled
        );
    }
}
