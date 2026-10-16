use std::collections::BTreeSet;

use crate::{fsx, manifest::Manifest, pkgsource::{self, Source}, registry::InstallMeta, update, Error, Result};

const RELEASES: &str = "https://github.com/Corfirean/coa-server-build/releases/download/server-";

#[derive(Debug)]
pub struct Selection {
    pub source: Source,
    pub latest_version: Option<String>,
}

pub fn select(meta: &InstallMeta, source: Source, key: &str) -> Result<Selection> {
    select_with(meta, source, &mut |source| pkgsource::fetch_manifest(source, key))
}

fn select_with(meta: &InstallMeta, source: Source, fetch: &mut impl FnMut(&Source) -> Result<(Manifest, Vec<u8>)>) -> Result<Selection> {
    let (manifest, _) = fetch(&source)?;
    if update::validate_candidate(meta, &manifest).is_ok() {
        return Ok(Selection { source, latest_version: None });
    }
    let original = update::validate_candidate(meta, &manifest).unwrap_err();
    if !matches!(&source, Source::Url(url) if url.trim_end_matches('/') == format!("{RELEASES}{}", manifest.version)) {
        return Err(original);
    }
    let Some(installed) = meta.core.version.as_deref().and_then(crate::manifest::parse_version) else { return Err(original); };
    if manifest.kind != crate::manifest::Kind::Update
        || manifest.compatibility.as_ref().is_none_or(|c| c.platform != if cfg!(windows) { "windows-x86_64" } else { "linux-x86_64" })
        || !manifest.compatible_with_manager(crate::MANAGER_VERSION)
        || crate::manifest::parse_version(&manifest.version).is_none_or(|version| version <= installed) {
        return Err(original);
    }
    let mut visited = BTreeSet::new();
    let Some(source) = find(meta, &manifest, fetch, &mut visited, 0)? else { return Err(original); };
    Ok(Selection { source, latest_version: Some(manifest.version) })
}

fn find(meta: &InstallMeta, manifest: &Manifest, fetch: &mut impl FnMut(&Source) -> Result<(Manifest, Vec<u8>)>, visited: &mut BTreeSet<String>, depth: usize) -> Result<Option<Source>> {
    if depth >= 8 || visited.len() >= 32 { return Ok(None); }
    let Some(compatibility) = &manifest.compatibility else { return Ok(None); };
    let Some(installed) = meta.core.version.as_deref().and_then(crate::manifest::parse_version) else { return Ok(None); };
    let target = crate::manifest::parse_version(&manifest.version).unwrap();
    let mut versions: Vec<_> = compatibility.source_versions.iter().filter_map(|version| {
        crate::manifest::parse_version(version).filter(|number| *number > installed && *number < target).map(|number| (number, version))
    }).collect();
    versions.sort_by(|left, right| right.0.cmp(&left.0));
    for (_, version) in versions {
        let Some(database) = compatibility.source_databases.get(version) else { continue; };
        if !visited.insert(version.clone()) || visited.len() > 32 { continue; }
        let source = Source::Url(format!("{RELEASES}{version}"));
        let (bridge, bytes) = fetch(&source)?;
        if bridge.version != *version || fsx::sha256_bytes(&bytes) != database.manifest_sha256 {
            return Err(Error::InvalidManifest("The intermediate update differs from the signed compatibility matrix.".into()));
        }
        if update::validate_candidate(meta, &bridge).is_ok() { return Ok(Some(source)); }
        if bridge.kind != crate::manifest::Kind::Update || !bridge.compatible_with_manager(crate::MANAGER_VERSION)
            || bridge.compatibility.as_ref().is_none_or(|c| c.platform != compatibility.platform) { continue; }
        if let Some(source) = find(meta, &bridge, fetch, visited, depth + 1)? { return Ok(Some(source)); }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use std::{fs, path::Path};

    fn package(version: &str, from: &str, digest: &str) -> serde_json::Value {
        serde_json::json!({"schema":1,"kind":"update","version":version,"core":{"commit":"a".repeat(40)},
            "builtAt":"test","minManagerVersion":"0.6.16","files":[],"migrations":[],"archive":null,
            "compatibility":{"schema":1,"platform":if cfg!(windows) {"windows-x86_64"} else {"linux-x86_64"},
                "coreCommit":"a".repeat(40),"moduleCommits":{"squid":"b".repeat(40)},"clientPatchVersion":"1.7",
                "sourceVersions":[from],"sourceDatabases":{from:{"manifestSha256":digest,"schemaSha256":null}}}})
    }

    fn meta(version: &str) -> InstallMeta {
        let mut meta = InstallMeta::new(crate::registry::InstallKind::New, Path::new("unused"));
        meta.core.version = Some(version.into());
        meta
    }

    fn signed_fixture(values: &[serde_json::Value], tamper: Option<&str>) -> (tempfile::TempDir, String) {
        let folder = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[37; 32]);
        for value in values {
            let version = value["version"].as_str().unwrap();
            let dir = folder.path().join(version);
            fs::create_dir(&dir).unwrap();
            let bytes = serde_json::to_vec(value).unwrap();
            fs::write(dir.join("manifest.json.sig"), STANDARD.encode(signing.sign(&bytes).to_bytes())).unwrap();
            fs::write(dir.join("manifest.json"), &bytes).unwrap();
            if tamper == Some(version) { fs::write(dir.join("manifest.json"), b"tampered").unwrap(); }
        }
        (folder, STANDARD.encode(signing.verifying_key().to_bytes()))
    }

    fn select_fixture(values: &[serde_json::Value], installed: &str, tamper: Option<&str>) -> Result<Selection> {
        let (folder, key) = signed_fixture(values, tamper);
        let latest = values.last().unwrap()["version"].as_str().unwrap();
        select_with(&meta(installed), Source::Url(format!("{RELEASES}{latest}")), &mut |source| {
            let Source::Url(url) = source else { unreachable!() };
            let version = url.strip_prefix(RELEASES).unwrap();
            pkgsource::fetch_manifest(&Source::Dir(folder.path().join(version)), &key)
        })
    }

    fn pair() -> Vec<serde_json::Value> {
        let bridge = package("1.1.0", "1.0.0", &"c".repeat(64));
        let digest = fsx::sha256_bytes(&serde_json::to_vec(&bridge).unwrap());
        vec![bridge, package("1.2.0", "1.1.0", &digest)]
    }

    #[test]
    fn finds_a_signed_intermediate_without_relaxing_candidate_validation() {
        let selection = select_fixture(&pair(), "1.0.0", None).unwrap();
        assert!(matches!(selection.source, Source::Url(url) if url.ends_with("1.1.0")));
        assert_eq!(selection.latest_version.as_deref(), Some("1.2.0"));
        let latest = Manifest::parse(&serde_json::to_vec(pair().last().unwrap()).unwrap()).unwrap();
        assert!(update::validate_candidate(&meta("1.0.0"), &latest).is_err());
    }

    #[test]
    fn direct_updates_and_current_versions_do_not_request_an_intermediate() {
        for installed in ["1.1.0", "1.2.0"] {
            let selection = select_fixture(&pair(), installed, None).unwrap();
            assert!(selection.latest_version.is_none());
        }
    }

    #[test]
    fn rejects_tampered_or_replaced_intermediate_manifests() {
        assert!(select_fixture(&pair(), "1.0.0", Some("1.1.0")).is_err());
        let mut values = pair();
        values[0]["builtAt"] = "replaced".into();
        assert!(select_fixture(&values, "1.0.0", None).unwrap_err().to_string().contains("differs"));
    }

    #[test]
    fn follows_multiple_tested_edges_and_rejects_missing_paths() {
        let mut values = pair();
        let digest = fsx::sha256_bytes(&serde_json::to_vec(values.last().unwrap()).unwrap());
        values.push(package("1.3.0", "1.2.0", &digest));
        let selection = select_fixture(&values, "1.0.0", None).unwrap();
        assert!(matches!(selection.source, Source::Url(url) if url.ends_with("1.1.0")));
        assert!(select_fixture(&values, "0.9.0", None).is_err());
        assert!(select_fixture(&values, "2.0.0", None).is_err());
    }

    #[test]
    fn never_routes_custom_sources_or_unknown_installations() {
        let values = pair();
        let (folder, key) = signed_fixture(&values, None);
        let mut calls = 0;
        let result = select_with(&meta("1.0.0"), Source::Dir(folder.path().join("1.2.0")), &mut |source| {
            calls += 1;
            pkgsource::fetch_manifest(source, &key)
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
        assert!(select_fixture(&values, "unknown", None).is_err());
    }

    #[test]
    fn rejects_incompatible_platforms_and_manager_versions() {
        for index in [0, 1] {
            for (field, value) in [("minManagerVersion", "99.0.0"), ("platform", "other-platform")] {
                let mut values = pair();
                if field == "platform" { values[index]["compatibility"][field] = value.into(); }
                else { values[index][field] = value.into(); }
                if index == 0 {
                    let digest = fsx::sha256_bytes(&serde_json::to_vec(&values[0]).unwrap());
                    values[1]["compatibility"]["sourceDatabases"]["1.1.0"]["manifestSha256"] = digest.into();
                }
                assert!(select_fixture(&values, "1.0.0", None).is_err());
            }
        }
    }
}
