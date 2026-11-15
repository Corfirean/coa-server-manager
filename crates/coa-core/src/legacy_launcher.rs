//! `Scripts/manage.py` as Managers up to 0.9.10 edited it.
//!
//! Those Managers ran the repack's Python launcher themselves and changed its text: an import fix for their bridge
//! and a realm-profile patch (database and realm row of the active realm). This Manager starts servers with
//! [`crate::launcher`] and never edits the file; it only has to recognise those edits, so that an update or a file
//! check treats a file one of them changed as unmodified. Nothing here writes a file.

use crate::error::{Error, Result};

const LAUNCHER_REALM_REPLACEMENTS: &[(&str, &str)] = &[
    ("(\"WorldDatabaseInfo\", \"acore_world\")", "(\"WorldDatabaseInfo\", \"acore_world_wildcard\" if coa_realm() == 'wildcard' else \"acore_world\")"),
    ("(\"CharacterDatabaseInfo\", \"acore_characters\")", "(\"CharacterDatabaseInfo\", \"acore_characters_wildcard\" if coa_realm() == 'wildcard' else \"acore_characters\")"),
    ("SET name='AzerothCore',address=", "SET name='{coa_realm_name()}',address="),
    ("WHERE id=1;", "WHERE id={coa_realm_id()};"),
    ("mysql(\"UPDATE acore_auth.realmlist SET flag=0", "mysql(f\"UPDATE acore_auth.realmlist SET flag=0"),
];

const LAUNCHER_REALM_INSERT: &str = "# coa-manager-realm-profiles-v1\ndef coa_realm():\n    path = ROOT / 'Settings/realm-profile.json'\n    return json.loads(path.read_text(encoding='utf-8')).get('active', 'coa') if path.exists() else 'coa'\n\ndef coa_realm_id():\n    return 2 if coa_realm() == 'wildcard' else 1\n\ndef coa_realm_name():\n    return 'Wildcard' if coa_realm() == 'wildcard' else 'Conquest of Azeroth'\n\n";

fn launcher_insert() -> String {
    LAUNCHER_REALM_INSERT
        .replace("profiles-v1", "profiles-v2")
        .replace(
            "return 2 if",
            "return 3 if coa_realm() == 'reborn' else 2 if",
        )
        .replace(
            "return 'Wildcard' if",
            "return 'Warcraft Reborn' if coa_realm() == 'reborn' else 'Wildcard' if",
        )
}

fn launcher_replacements() -> Vec<(&'static str, String)> {
    LAUNCHER_REALM_REPLACEMENTS.iter().map(|(from, to)| (*from, to
        .replace("\"acore_world_wildcard\" if", "\"acore_world_reborn\" if coa_realm() == 'reborn' else \"acore_world_wildcard\" if")
        .replace("\"acore_characters_wildcard\" if", "\"acore_characters_reborn\" if coa_realm() == 'reborn' else \"acore_characters_wildcard\" if"))).collect()
}

/// The realm-profile patch older Managers wrote.
pub(crate) fn patch_launcher(source: &str) -> Result<String> {
    if source.contains("# coa-manager-realm-profiles-v2") {
        return Ok(source.into());
    }
    if source.contains("# coa-manager-realm-profiles-v1") {
        let original = unpatch_launcher(source)
            .ok_or_else(|| Error::Invalid("Unrecognised realm launcher patch.".into()))?;
        return patch_launcher(&original);
    }
    let mut out = source.to_string();
    for (from, to) in launcher_replacements() {
        if !out.contains(from) {
            return Err(Error::Invalid(
                "This launcher does not support realm profiles. Update the server package first."
                    .into(),
            ));
        }
        out = out.replace(from, &to);
    }
    // Insert before the entry point so ROOT, json and functions are all defined when called.
    let marker = "if __name__ == \"__main__\":";
    if !out.contains(marker) {
        return Err(Error::Invalid(
            "Unrecognised server launcher entry point.".into(),
        ));
    }
    Ok(out.replace(marker, &format!("{}{marker}", launcher_insert())))
}

/// Reverse only our exact realm transformation; callers must verify the recovered original hash.
pub(crate) fn unpatch_launcher(source: &str) -> Option<String> {
    let (insert, replacements) = if source.contains(&launcher_insert()) {
        (launcher_insert(), launcher_replacements())
    } else if source.contains(LAUNCHER_REALM_INSERT) {
        (
            LAUNCHER_REALM_INSERT.into(),
            LAUNCHER_REALM_REPLACEMENTS
                .iter()
                .map(|(a, b)| (*a, b.to_string()))
                .collect(),
        )
    } else {
        return None;
    };
    let mut original = source.replacen(&insert, "", 1);
    for (from, to) in &replacements {
        original = original.replace(to, from);
    }
    let mut check = original.clone();
    for (from, to) in &replacements {
        check = check.replace(from, to);
    }
    check = check.replace(
        "if __name__ == \"__main__\":",
        &format!("{insert}if __name__ == \"__main__\":"),
    );
    (check == source).then_some(original)
}

/// The import fix older Managers wrote (needed to run manage.py through their bridge).
pub(crate) fn patch_launcher_imports(text: &str) -> Result<String> {
    let line = "sys.path.insert(0, str(Path(__file__).resolve().parent))";
    if !text.contains("from squid_playerbots import") || text.contains(line) {
        return Ok(text.into());
    }
    let root = "ROOT = Path(__file__).resolve().parents[1]";
    if !text.contains(root) || !text.lines().any(|line| line.trim() == "import sys") {
        return Err(Error::Invalid(
            "The server launcher cannot load its integration scripts. Repair its program files."
                .into(),
        ));
    }
    Ok(text.replacen(root, &format!("{root}\n{line}"), 1))
}

pub(crate) fn launcher_matches(signed: &str, actual: &[u8]) -> bool {
    if signed.as_bytes() == actual {
        return true;
    }
    if patch_launcher_imports(signed).is_ok_and(|s| s.as_bytes() == actual) {
        return true;
    }
    patch_launcher(signed).is_ok_and(|s| {
        s.as_bytes() == actual || patch_launcher_imports(&s).is_ok_and(|p| p.as_bytes() == actual)
    })
}

/// Recover legacy metadata only when undoing supported Manager edits reproduces the recorded hash.
pub(crate) fn launcher_matches_recorded(recorded: &str, actual: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(actual) else {
        return false;
    };
    let without_import = text.replacen(
        "ROOT = Path(__file__).resolve().parents[1]\nsys.path.insert(0, str(Path(__file__).resolve().parent))",
        "ROOT = Path(__file__).resolve().parents[1]", 1,
    );
    for candidate in [
        Some(without_import.clone()),
        unpatch_launcher(&without_import),
    ]
    .into_iter()
    .flatten()
    {
        if crate::fsx::sha256_bytes(candidate.as_bytes()).eq_ignore_ascii_case(recorded)
            && launcher_matches(&candidate, actual)
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx;
    use std::collections::BTreeMap;
    use std::fs;

    #[test]
    fn old_launcher_upgrades_and_both_patch_versions_are_reversible() {
        let original = "(\"WorldDatabaseInfo\", \"acore_world\")\n(\"CharacterDatabaseInfo\", \"acore_characters\")\nSET name='AzerothCore',address=\nWHERE id=1;\nmysql(\"UPDATE acore_auth.realmlist SET flag=0\nif __name__ == \"__main__\":\n";
        let mut old = original.to_string();
        for (from, to) in LAUNCHER_REALM_REPLACEMENTS {
            old = old.replace(from, to);
        }
        old = old.replace(
            "if __name__ == \"__main__\":",
            &format!("{LAUNCHER_REALM_INSERT}if __name__ == \"__main__\":"),
        );
        assert_eq!(unpatch_launcher(&old).as_deref(), Some(original));
        let new = patch_launcher(&old).unwrap();
        assert!(new.contains("acore_world_reborn"));
        assert!(new.contains("return 3 if coa_realm() == 'reborn'"));
        assert!(new.contains("return 'Warcraft Reborn' if"));
        assert_eq!(patch_launcher(&new).unwrap(), new);
        assert_eq!(unpatch_launcher(&new).as_deref(), Some(original));
    }

    #[test]
    fn unknown_launcher_is_refused_without_partial_patching() {
        assert!(patch_launcher("print('custom launcher')").is_err());
    }

    #[test]
    fn launcher_import_fix_is_idempotent_and_retains_other_code() {
        let text = "import sys\nROOT = Path(__file__).resolve().parents[1]\nfrom squid_playerbots import validate_bots\ncustom = 42\n";
        let fixed = patch_launcher_imports(text).unwrap();
        assert!(fixed.contains("sys.path.insert(0, str(Path(__file__).resolve().parent))"));
        assert!(fixed.ends_with("custom = 42\n"));
        assert_eq!(patch_launcher_imports(&fixed).unwrap(), fixed);
    }

    #[test]
    fn a_launcher_patched_by_an_older_manager_stays_pristine_but_external_edits_conflict() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("server");
        let source = "(\"WorldDatabaseInfo\", \"acore_world\")\n(\"CharacterDatabaseInfo\", \"acore_characters\")\nSET name='AzerothCore',address=\nWHERE id=1;\nmysql(\"UPDATE acore_auth.realmlist SET flag=0\nif __name__ == \"__main__\":\n";
        fsx::atomic_write(&root.join("Scripts/manage.py"), source.as_bytes()).unwrap();
        // What an older Manager left: the realm patch applied and its hash recorded as the pristine one.
        let patched = patch_launcher(source).unwrap();
        fsx::atomic_write(&root.join("Scripts/manage.py"), patched.as_bytes()).unwrap();
        let mut meta = crate::registry::InstallMeta::new(crate::registry::InstallKind::New, &root);
        meta.original_hashes.insert("Scripts/manage.py".into(), fsx::sha256_bytes(patched.as_bytes()));
        crate::registry::MetaDir::create(&root, &meta).unwrap();
        assert!(crate::diag::verify_managed(&root, &meta).is_empty());
        let manifest: crate::manifest::Manifest = serde_json::from_value(serde_json::json!({
            "schema": 1, "kind": "update", "version": "2.0.0", "core": {"commit": null}, "builtAt": "fixture", "minManagerVersion": "0.1.0",
            "files": [{"path":"Scripts/manage.py","sha256":fsx::sha256_bytes(b"next release"),"size":12,"owner":"core","policy":"replace"}]
        })).unwrap();
        assert_eq!(
            crate::update::plan(&root, &meta, &manifest, &BTreeMap::new(), None).unwrap()[0].action,
            crate::update::Action::Replace
        );
        let mut modified = fs::read(root.join("Scripts/manage.py")).unwrap();
        modified.extend_from_slice(b"# custom edit\n");
        fsx::atomic_write(&root.join("Scripts/manage.py"), &modified).unwrap();
        assert_eq!(
            crate::update::plan(&root, &meta, &manifest, &BTreeMap::new(), None).unwrap()[0].action,
            crate::update::Action::Conflict
        );
    }
}
