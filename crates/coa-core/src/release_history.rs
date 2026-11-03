//! Read-only, bounded canonical histories. Offline fallback is also bundled in the UI.
use serde_json::Value;
use std::{io::Read, path::Path, time::Duration};

const LIMIT: u64 = 2 * 1024 * 1024;

fn repository(component: &str) -> Result<(&'static str, &'static str), String> {
    match component {
        "server" => Ok(("Corfirean/coa-server-build", "main")),
        "manager" => Ok(("Corfirean/coa-server-manager", "master")),
        _ => Err("Unknown release component".into()),
    }
}

fn validate(bytes: &[u8], component: &str) -> Result<Value, String> {
    let (repo, _) = repository(component)?;
    if bytes.len() as u64 > LIMIT {
        return Err("Release history is too large".into());
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if value["schema"] != 1 || value["component"] != component || value["repository"] != repo {
        return Err("Release history identity mismatch".into());
    }
    let releases = value["releases"].as_array().ok_or("Missing releases")?;
    if releases.len() > 2000 {
        return Err("Too many releases".into());
    }
    for release in releases {
        if release.get("prerelease").is_some_and(|v| !v.is_boolean())
            || release.get("experimentalFeatures").is_some_and(|v| {
                v.as_array()
                    .is_none_or(|items| items.iter().any(|item| !item.is_string()))
            })
        {
            return Err("Invalid release flags".into());
        }
        for field in ["tag", "date"] {
            if release[field]
                .as_str()
                .is_none_or(|s| s.is_empty() || s.len() > 100)
            {
                return Err("Invalid release metadata".into());
            }
        }
        let version = release["version"].as_str().ok_or("Missing version")?;
        if version.len() > 80
            || !version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
        {
            return Err("Invalid version".into());
        }
        let notes = release["notes"].as_array().ok_or("Missing notes")?;
        if notes.len() > 500
            || notes
                .iter()
                .any(|n| n.as_str().is_none_or(|s| s.len() > 10000))
        {
            return Err("Invalid release notes".into());
        }
    }
    Ok(value)
}

pub fn load(component: &str, cache_dir: &Path) -> Result<Value, String> {
    load_for_target(component, cache_dir, None)
}

pub fn load_for_target(
    component: &str,
    cache_dir: &Path,
    target: Option<&str>,
) -> Result<Value, String> {
    let (repo, branch) = repository(component)?;
    load_with_remote(component, cache_dir, target, || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .get(format!(
                "https://raw.githubusercontent.com/{repo}/{branch}/changelog/history.json"
            ))
            .query(&[("target", target.unwrap_or(""))])
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        response
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Ok(bytes)
    })
}

fn load_with_remote(
    component: &str,
    cache_dir: &Path,
    target: Option<&str>,
    remote: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<Value, String> {
    repository(component)?;
    let cache = cache_dir.join(format!("{component}.json"));
    let cached = || {
        let mut bytes = Vec::new();
        std::fs::File::open(&cache)
            .ok()?
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        validate(&bytes, component).ok()
    };
    if cache
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(3600))
    {
        if let Some(mut value) = cached() {
            if target.is_none_or(|target| {
                value["releases"].as_array().is_some_and(|releases| {
                    releases
                        .iter()
                        .any(|release| release["version"].as_str() == Some(target))
                })
            }) {
                value["source"] = "cache".into();
                return Ok(value);
            }
        }
    }
    let fetched = (|| {
        let bytes = remote()?;
        let mut value = validate(&bytes, component)?;
        if std::fs::create_dir_all(cache_dir).is_ok() {
            let _ = crate::fsx::atomic_write(&cache, &bytes);
        }
        value["source"] = "remote".into();
        Ok(value)
    })();
    fetched.or_else(|error| {
        cached()
            .map(|mut value| {
                value["source"] = "cache".into();
                value
            })
            .ok_or(error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_both_complete_bundled_canonical_histories() {
        assert!(validate(include_bytes!("../../../changelog/history.json"), "manager").is_ok());
        assert!(validate(
            include_bytes!("../../../src/data/server-history.json"),
            "server"
        )
        .is_ok());
    }
    #[test]
    fn rejects_cross_component_and_untrusted_components() {
        let bytes = br#"{"schema":1,"component":"manager","repository":"Corfirean/coa-server-manager","releases":[]}"#;
        assert!(validate(bytes, "manager").is_ok());
        assert!(validate(bytes, "server").is_err());
        assert!(repository("../../other").is_err());
        assert!(validate(&vec![b' '; LIMIT as usize + 1], "manager").is_err());
    }

    #[test]
    fn rejects_malformed_records_before_the_ui_can_use_them() {
        let mut value = serde_json::json!({"schema": 1, "component": "server",
        "repository": "Corfirean/coa-server-build", "releases": [{
            "version": "0.261028.83", "tag": "0.261028.83", "date": "2026-10-28",
            "notes": ["Reviewed note"]
        }]});
        assert!(validate(&serde_json::to_vec(&value).unwrap(), "server").is_ok());
        value["releases"][0]["date"] = Value::Null;
        assert!(validate(&serde_json::to_vec(&value).unwrap(), "server").is_err());
        value["releases"][0]["date"] = "2026-10-28".into();
        value["releases"][0]["notes"] = serde_json::json!([{"html": "not a text note"}]);
        assert!(validate(&serde_json::to_vec(&value).unwrap(), "server").is_err());
    }

    #[test]
    fn available_update_bypasses_fresh_cache_missing_the_target_and_keeps_offline_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let feed = |version: &str| {
            serde_json::to_vec(&serde_json::json!({
                "schema":1,"component":"manager","repository":"Corfirean/coa-server-manager",
                "releases":[{"version":version,"tag":format!("v{version}"),"date":"2026-10-30",
                             "notes":["Visible update notes"]}]
            }))
            .unwrap()
        };
        std::fs::write(dir.path().join("manager.json"), feed("0.9.4")).unwrap();
        let value =
            load_with_remote("manager", dir.path(), Some("0.9.5"), || Ok(feed("0.9.5"))).unwrap();
        assert_eq!(value["source"], "remote");
        assert_eq!(value["releases"][0]["version"], "0.9.5");
        let cached = load_with_remote("manager", dir.path(), Some("0.9.5"), || {
            panic!("fresh target is cached")
        })
        .unwrap();
        assert_eq!(cached["source"], "cache");
        let offline = load_with_remote("manager", dir.path(), Some("0.9.6"), || {
            Err("offline".into())
        })
        .unwrap();
        assert_eq!(offline["releases"][0]["version"], "0.9.5");
    }

    #[test]
    fn fresh_valid_cache_is_available_without_network() {
        let dir = std::env::temp_dir().join(format!("coa-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manager.json"), br#"{"schema":1,"component":"manager","repository":"Corfirean/coa-server-manager","releases":[]}"#).unwrap();
        let result = load("manager", &dir).unwrap();
        assert_eq!(result["source"], "cache");
        std::fs::remove_file(dir.join("manager.json")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
