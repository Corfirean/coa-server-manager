use std::path::Path;

pub fn close_to_tray(dir: &Path) -> coa_core::Result<bool> {
    match std::fs::read(dir.join("close-to-tray.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub fn save(dir: &Path, enabled: bool) -> coa_core::Result<()> {
    coa_core::fsx::atomic_write_json(&dir.join("close-to-tray.json"), &enabled)
}

/// Clear the game's cache of server data before every Play (on unless the player turned it off).
pub fn clear_cache_on_play(dir: &Path) -> bool {
    std::fs::read(dir.join("clear-game-cache.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(true)
}

pub fn save_clear_cache_on_play(dir: &Path, enabled: bool) -> coa_core::Result<()> {
    coa_core::fsx::atomic_write_json(&dir.join("clear-game-cache.json"), &enabled)
}
