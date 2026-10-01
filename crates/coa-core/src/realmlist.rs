//! Named realmlists for the game client ("Solo" for your own server, "PTR" for the shared one, ...). Choosing one writes its
//! text into the client's `Data/<locale>/realmlist.wtf` files; the file that was there is saved first. The profiles live
//! in the Manager's folder for the server, never in the client.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::client;
use crate::error::{Error, Result};
use crate::fsx;

const FILE: &str = "realmlists.json";
const MAX_PROFILES: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// The text of `realmlist.wtf` (normalised: `set <name> <value>` lines).
    pub data: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    profiles: Vec<Profile>,
}

#[derive(Debug, Serialize)]
pub struct View {
    pub profiles: Vec<Profile>,
    /// The profile whose text every realmlist file of the client currently holds.
    pub active: Option<String>,
}

fn compact(text: &str) -> String {
    text.lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// Accept either the whole file text or just an address ("play.example.com", "192.168.0.5:3724"), and make sure that
/// what is written into the client is nothing but `set <name> <value>` lines.
pub fn normalize(data: &str) -> Result<String> {
    let bad = |m: &str| Err(Error::Invalid(m.into()));
    let text = data.trim();
    if text.is_empty() {
        return bad("Enter the realmlist text or an address.");
    }
    if text.len() > 2000 {
        return bad("That text is too long for a realmlist.");
    }
    let lines: Vec<String> = if !text.contains(char::is_whitespace) {
        vec![format!("set realmlist {text}")]
    } else {
        text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
    };
    for line in &lines {
        let mut parts = line.splitn(3, char::is_whitespace);
        let (set, key, value) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or("").trim());
        if !set.eq_ignore_ascii_case("set") || key.is_empty() || value.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return bad("Each line must look like: set realmlist 127.0.0.1");
        }
        if !value.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            return bad("The realmlist may only use plain letters, digits and punctuation.");
        }
        if key.eq_ignore_ascii_case("realmlist") && !value.trim_matches('"').chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_')) {
            return bad("That address is not valid.");
        }
    }
    Ok(lines.join("\r\n") + "\r\n")
}

fn slug(name: &str) -> String {
    let s: String = name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "realm".into() } else { s }
}

fn load(meta: &Path, client_dir: Option<&Path>) -> Store {
    let mut store: Store = fsx::read_json(&meta.join(FILE)).unwrap_or_default();
    if store.profiles.is_empty() {
        // First use: "Solo" for this computer, and what the client held before so that it can be switched back to.
        store.profiles.push(Profile { id: "solo".into(), name: "Solo".into(), data: "set realmlist 127.0.0.1\r\n".into() });
        if let Some(files) = client_dir.map(client::realmlist_files) {
            if let Some(text) = files.iter().find_map(|f| fs::read_to_string(f).ok()) {
                if let Ok(data) = normalize(&text) {
                    if compact(&data) != compact(&store.profiles[0].data) {
                        store.profiles.push(Profile { id: "previous".into(), name: "Previous".into(), data });
                    }
                }
            }
        }
        let _ = fsx::atomic_write_json(&meta.join(FILE), &store);
    }
    store
}

fn save_store(meta: &Path, store: &Store) -> Result<()> {
    fsx::atomic_write_json(&meta.join(FILE), store)
}

pub fn view(meta: &Path, client_dir: Option<&Path>) -> View {
    let store = load(meta, client_dir);
    let active = client_dir.and_then(|c| {
        let files = client::realmlist_files(c);
        let texts: Vec<String> = files.iter().filter_map(|f| fs::read_to_string(f).ok()).map(|t| compact(&t)).collect();
        let first = texts.first()?;
        if texts.iter().any(|t| t != first) {
            return None;
        }
        store.profiles.iter().find(|p| compact(&p.data) == *first).map(|p| p.id.clone())
    });
    View { profiles: store.profiles, active }
}

/// Add a profile (`id` None) or change one. Names are 1-32 characters and unique regardless of capitals.
pub fn save_profile(meta: &Path, client_dir: Option<&Path>, id: Option<&str>, name: &str, data: &str) -> Result<Profile> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err(Error::Invalid("The name must be 1 to 32 characters.".into()));
    }
    let data = normalize(data)?;
    let mut store = load(meta, client_dir);
    if store.profiles.iter().any(|p| p.name.eq_ignore_ascii_case(name) && Some(p.id.as_str()) != id) {
        return Err(Error::Invalid("A realmlist with that name already exists.".into()));
    }
    let profile = match id {
        Some(id) => {
            let p = store.profiles.iter_mut().find(|p| p.id == id).ok_or_else(|| Error::Invalid("That realmlist does not exist.".into()))?;
            p.name = name.to_string();
            p.data = data;
            p.clone()
        }
        None => {
            if store.profiles.len() >= MAX_PROFILES {
                return Err(Error::Invalid("There are too many realmlists; remove one first.".into()));
            }
            let mut new_id = slug(name);
            while store.profiles.iter().any(|p| p.id == new_id) {
                new_id.push('-');
            }
            let p = Profile { id: new_id, name: name.to_string(), data };
            store.profiles.push(p.clone());
            p
        }
    };
    save_store(meta, &store)?;
    Ok(profile)
}

pub fn delete_profile(meta: &Path, client_dir: Option<&Path>, id: &str) -> Result<()> {
    let mut store = load(meta, client_dir);
    if store.profiles.len() <= 1 {
        return Err(Error::Invalid("The last realmlist cannot be removed.".into()));
    }
    store.profiles.retain(|p| p.id != id);
    save_store(meta, &store)
}

/// Write the profile into the client; returns the files that changed (each was saved to the backup folder first).
pub fn activate(meta: &Path, client_dir: &Path, id: &str) -> Result<Vec<String>> {
    let store = load(meta, Some(client_dir));
    let profile = store.profiles.iter().find(|p| p.id == id).ok_or_else(|| Error::Invalid("That realmlist does not exist.".into()))?;
    client::write_realmlist(client_dir, meta, &profile.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_client(d: &Path, host: &str) -> std::path::PathBuf {
        let c = d.join("wow");
        for loc in ["enUS", "ruRU"] {
            fs::create_dir_all(c.join("Data").join(loc)).unwrap();
            fs::write(c.join("Data").join(loc).join("realmlist.wtf"), format!("set realmlist {host}\r\nset patchlist x.example\r\n")).unwrap();
        }
        fs::write(c.join("Ascension.exe"), b"exe").unwrap();
        c
    }

    #[test]
    fn an_address_or_whole_lines_are_accepted_and_anything_else_is_not() {
        assert_eq!(normalize("play.example.com").unwrap(), "set realmlist play.example.com\r\n");
        assert_eq!(normalize("SET realmlist 192.168.0.5:3724\nset patchlist x.example\n").unwrap(), "SET realmlist 192.168.0.5:3724\r\nset patchlist x.example\r\n");
        for bad in ["", "   ", "rm -rf /", "set realmlist bad host;calc", "set realmlist", "x\ny", "set realmlist a\"b", &"a".repeat(2100)] {
            assert!(normalize(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_first_use_offers_solo_and_what_the_client_had_and_the_active_one_is_recognised() {
        let d = tempfile::tempdir().unwrap();
        let (c, meta) = (fake_client(d.path(), "ptr.example.org"), d.path().join("meta"));
        fs::create_dir_all(&meta).unwrap();
        let v = view(&meta, Some(&c));
        assert_eq!(v.profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Solo", "Previous"]);
        assert_eq!(v.active.as_deref(), Some("previous"), "the files hold exactly what was captured");
        assert!(activate(&meta, &c, "previous").unwrap().is_empty(), "already what the files hold");
        assert_eq!(fs::read_to_string(c.join("Data/enUS/realmlist.wtf")).unwrap(), "set realmlist ptr.example.org\r\nset patchlist x.example\r\n");
    }

    #[test]
    fn switching_writes_every_locale_saves_the_old_file_and_changes_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let (c, meta) = (fake_client(d.path(), "ptr.example.org"), d.path().join("meta"));
        fs::create_dir_all(&meta).unwrap();
        fs::write(c.join("WTF.txt"), b"mine").unwrap();
        let ptr = save_profile(&meta, Some(&c), None, "PTR", "ptr.example.org").unwrap();
        let changed = activate(&meta, &c, "solo").unwrap();
        assert_eq!(changed.len(), 2);
        for loc in ["enUS", "ruRU"] {
            assert_eq!(fs::read_to_string(c.join("Data").join(loc).join("realmlist.wtf")).unwrap(), "set realmlist 127.0.0.1\r\n");
        }
        assert_eq!(view(&meta, Some(&c)).active.as_deref(), Some("solo"));
        assert_eq!(fs::read_dir(meta.join("backups/client")).unwrap().count(), 2, "each replaced file was saved");
        activate(&meta, &c, &ptr.id).unwrap();
        assert_eq!(view(&meta, Some(&c)).active.as_deref(), Some(ptr.id.as_str()));
        assert_eq!(fs::read(c.join("WTF.txt")).unwrap(), b"mine");
    }

    #[test]
    fn names_are_unique_profiles_can_be_edited_and_the_last_one_stays() {
        let d = tempfile::tempdir().unwrap();
        let meta = d.path().join("meta");
        fs::create_dir_all(&meta).unwrap();
        let a = save_profile(&meta, None, None, "PTR", "ptr.example.org").unwrap();
        assert!(save_profile(&meta, None, None, "ptr", "other.example.org").is_err(), "names are unique regardless of capitals");
        let edited = save_profile(&meta, None, Some(&a.id), "PTR 2", "ptr2.example.org").unwrap();
        assert_eq!((edited.id.as_str(), edited.data.as_str()), (a.id.as_str(), "set realmlist ptr2.example.org\r\n"));
        assert!(save_profile(&meta, None, None, "", "x.example.org").is_err());
        delete_profile(&meta, None, &a.id).unwrap();
        assert!(delete_profile(&meta, None, "solo").is_err(), "one profile must remain");
        assert_eq!(view(&meta, None).profiles.len(), 1);
    }
}
