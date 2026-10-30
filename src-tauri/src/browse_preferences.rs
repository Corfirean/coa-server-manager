use std::path::Path;

pub fn enabled(dir: &Path) -> coa_core::Result<bool> {
    match std::fs::read(dir.join("public-server-browsing.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e.into()),
    }
}

pub fn save(dir: &Path, enabled: bool) -> coa_core::Result<()> {
    coa_core::fsx::atomic_write_json(&dir.join("public-server-browsing.json"), &enabled)
}

pub fn require_enabled(dir: &Path) -> coa_core::Result<()> {
    if enabled(dir)? {
        Ok(())
    } else {
        Err(coa_core::Error::Invalid(
            "Public server browsing is disabled in Settings.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_and_corrupt_preferences_block_browsing() {
        let dir = std::env::temp_dir().join(format!(
            "coa-browse-prefs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(enabled(&dir).unwrap());
        save(&dir, false).unwrap();
        assert!(!enabled(&dir).unwrap());
        assert!(require_enabled(&dir).is_err());
        save(&dir, true).unwrap();
        assert!(require_enabled(&dir).is_ok());
        let file = dir.join("public-server-browsing.json");
        std::fs::write(&file, b"broken").unwrap();
        assert!(require_enabled(&dir).is_err());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
