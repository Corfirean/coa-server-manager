use crate::{
    manifest::Manifest,
    pkgsource::{self, Source},
    registry::InstallMeta,
    update, Result,
};

#[derive(Debug)]
pub struct Selection {
    pub source: Source,
    // Retained for the frontend response shape; direct updates never offer a bridge.
    pub latest_version: Option<String>,
}

pub fn select(meta: &InstallMeta, source: Source, key: &str) -> Result<Selection> {
    select_with(meta, source, &mut |source| {
        pkgsource::fetch_manifest(source, key)
    })
}

fn select_with(
    meta: &InstallMeta,
    source: Source,
    fetch: &mut impl FnMut(&Source) -> Result<(Manifest, Vec<u8>)>,
) -> Result<Selection> {
    let (manifest, _) = fetch(&source)?;
    update::validate_candidate(meta, &manifest)?;
    Ok(Selection {
        source,
        latest_version: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use std::{fs, path::Path};

    fn package() -> serde_json::Value {
        serde_json::json!({"schema":1,"kind":"update","version":"0.261015.58","core":{"commit":"a".repeat(40)},
            "builtAt":"test","minManagerVersion":"0.6.16","files":[],"migrations":[],"archive":null,
            "compatibility":{"schema":1,"platform":if cfg!(windows) {"windows-x86_64"} else {"linux-x86_64"},
                "coreCommit":"a".repeat(40),"moduleCommits":{"squid":"b".repeat(40)},"clientPatchVersion":"1.7",
                "sourceVersions":["0.261013.54"],"sourceDatabases":{"0.261013.54":{"manifestSha256":"c".repeat(64),"schemaSha256":null}}}})
    }

    fn meta(version: &str) -> InstallMeta {
        let mut meta = InstallMeta::new(crate::registry::InstallKind::New, Path::new("unused"));
        meta.core.version = Some(version.into());
        meta
    }

    fn fixture(value: &serde_json::Value, tamper: bool) -> (tempfile::TempDir, String) {
        let folder = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[37; 32]);
        let bytes = serde_json::to_vec(value).unwrap();
        fs::write(
            folder.path().join("manifest.json.sig"),
            STANDARD.encode(signing.sign(&bytes).to_bytes()),
        )
        .unwrap();
        fs::write(
            folder.path().join("manifest.json"),
            if tamper {
                b"tampered".as_slice()
            } else {
                &bytes
            },
        )
        .unwrap();
        (folder, STANDARD.encode(signing.verifying_key().to_bytes()))
    }

    #[test]
    fn every_known_older_version_selects_the_latest_signed_package_once() {
        for installed in [
            "0.2.0",
            "0.261004.20",
            "0.261005.0",
            "0.261007.24",
            "0.261011.800006",
            "0.261013.54",
            "0.261015.58",
        ] {
            let (folder, key) = fixture(&package(), false);
            for kind in [
                crate::registry::InstallKind::New,
                crate::registry::InstallKind::Imported,
            ] {
                let mut installation = meta(installed);
                installation.kind = kind;
                let mut calls = 0;
                let selected = select_with(
                    &installation,
                    Source::Dir(folder.path().into()),
                    &mut |source| {
                        calls += 1;
                        pkgsource::fetch_manifest(source, &key)
                    },
                )
                .unwrap();
                assert!(matches!(selected.source, Source::Dir(path) if path == folder.path()));
                assert!(selected.latest_version.is_none());
                assert_eq!(calls, 1, "no intermediate package fetches for {installed}");
            }
        }
    }

    #[test]
    fn unknown_versions_and_downgrades_remain_blocked() {
        let (folder, key) = fixture(&package(), false);
        for installed in ["unknown", "a65be06ec62f4", "0.261016.59"] {
            assert!(select(&meta(installed), Source::Dir(folder.path().into()), &key).is_err());
        }
    }

    #[test]
    fn signed_target_is_required_even_for_unlisted_old_versions() {
        let (folder, key) = fixture(&package(), true);
        assert!(select(
            &meta("0.261004.20"),
            Source::Dir(folder.path().into()),
            &key
        )
        .is_err());
    }

    #[test]
    fn platform_and_manager_requirements_are_still_enforced() {
        for (field, value) in [
            ("minManagerVersion", "99.0.0"),
            ("platform", "other-platform"),
        ] {
            let mut target = package();
            if field == "platform" {
                target["compatibility"][field] = value.into();
            } else {
                target[field] = value.into();
            }
            let (folder, key) = fixture(&target, false);
            assert!(select(
                &meta("0.261004.20"),
                Source::Dir(folder.path().into()),
                &key
            )
            .is_err());
        }
    }
}
