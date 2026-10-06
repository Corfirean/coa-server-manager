//! Release engineering: ordered SQL collection, cumulative update packages and a clean base tree.
//! Used by the `coa-release` command line tool (CI and the maintainer's machine); never by the app at runtime.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::fsx;
use crate::manifest::{Kind, Manifest, Migration, Revision};
use crate::package::{self, BuildOptions};

#[derive(Debug, Clone)]
pub struct SqlFile {
    pub db: String,
    pub id: String,
    /// Path relative to the checkout it came from (used only for ordering and display).
    pub origin: String,
    pub abs: PathBuf,
    pub sha256: String,
}

fn sql_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "sql").unwrap_or(false)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// The order in which files were first added to the repository (a pending script may rely on a table a module
/// created earlier). Files git does not know come last, in name order.
fn introduction_rank(repo: &Path, pathspecs: &[&str]) -> std::collections::HashMap<String, usize> {
    let mut args = vec!["log", "--reverse", "--diff-filter=A", "--name-only", "--pretty=format:", "--"];
    args.extend_from_slice(pathspecs);
    let out = Command::new("git").current_dir(repo).args(&args).output();
    let mut rank = std::collections::HashMap::new();
    if let Ok(o) = out {
        for (i, line) in String::from_utf8_lossy(&o.stdout).lines().filter(|l| l.ends_with(".sql")).enumerate() {
            rank.entry(line.to_string()).or_insert(i);
        }
    }
    rank
}

/// Every SQL script of a core checkout (released + pending updates, module base and update scripts), in application order.
pub fn collect_core_sql(core: &Path) -> Result<Vec<SqlFile>> {
    let mut out = Vec::new();
    let mut push = |db: &str, id: String, abs: PathBuf, origin: String| -> Result<()> {
        let sha256 = fsx::sha256_file(&abs)?;
        out.push(SqlFile { db: db.into(), id, origin, abs, sha256 });
        Ok(())
    };
    for (kind, dirs) in [("auth", ["db_auth", "pending_db_auth"]), ("characters", ["db_characters", "pending_db_characters"]), ("world", ["db_world", "pending_db_world"])] {
        for d in dirs {
            for f in sql_in(&core.join("data/sql/updates").join(d)) {
                let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
                let rel = format!("data/sql/updates/{d}/{}", f.file_name().unwrap().to_string_lossy());
                push(kind, stem, f, rel)?;
            }
        }
    }
    if let Ok(rd) = fs::read_dir(core.join("modules")) {
        let mut mods: Vec<_> = rd.flatten().collect();
        mods.sort_by_key(|e| e.file_name());
        for m in mods {
            let name = m.file_name().to_string_lossy().into_owned();
            let mname = name.replace('-', "_");
            for kind in ["auth", "characters", "world"] {
                let base = m.path().join(format!("data/sql/db-{kind}"));
                for f in sql_in(&base.join("base")) {
                    let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
                    let rel = format!("modules/{name}/data/sql/db-{kind}/base/{}", f.file_name().unwrap().to_string_lossy());
                    push(kind, format!("mod_{mname}__base__{stem}"), f, rel)?;
                }
                for f in sql_in(&base) {
                    let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
                    let rel = format!("modules/{name}/data/sql/db-{kind}/{}", f.file_name().unwrap().to_string_lossy());
                    push(kind, format!("mod_{mname}__{stem}"), f, rel)?;
                }
            }
        }
    }
    let rank = introduction_rank(core, &["data/sql/updates", "modules"]);
    out.sort_by_key(|f| rank.get(&f.origin).copied().unwrap_or(usize::MAX));
    Ok(out)
}

/// SQL a bots module checkout ships for the characters database (its `dist/sql`).
pub fn collect_bots_sql(bots: &Path) -> Result<Vec<SqlFile>> {
    let mut out = Vec::new();
    for f in sql_in(&bots.join("dist/sql")) {
        let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
        out.push(SqlFile { db: "characters".into(), id: format!("mod_coa_playerbots__{stem}"), origin: format!("dist/sql/{stem}.sql"), sha256: fsx::sha256_file(&f)?, abs: f });
    }
    Ok(out)
}

fn to_migrations(files: &[SqlFile]) -> Vec<Migration> {
    files.iter().map(|f| Migration { compatible_sha256: vec![], id: f.id.clone(), db: f.db.clone(), sha256: f.sha256.clone(), destructive: false }).collect()
}

pub struct UpdateParams<'a> {
    /// Directory laid out like the server folder that holds the release's files (binaries, dist configs, addon...).
    pub tree: &'a Path,
    /// Manifest of the base package this update is cumulative against.
    pub base_manifest: &'a Manifest,
    pub sql: &'a [SqlFile],
    pub out: &'a Path,
    pub version: String,
    pub core_commit: Option<String>,
    pub bots_commit: Option<String>,
    pub part_size: u64,
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            walk(root, &p, out)?;
        } else {
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

/// Build a cumulative update: every file of `tree` that differs from the base, plus every SQL script. Unsigned.
pub fn pack_update(p: &UpdateParams, progress: &dyn Fn(&str)) -> Result<Manifest> {
    let base: BTreeMap<String, &str> = p.base_manifest.files.iter().map(|f| (f.path.to_lowercase(), f.sha256.as_str())).collect();
    let work = p.out.with_file_name(format!("{}-staging", p.out.file_name().unwrap().to_string_lossy()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;

    let mut rels = Vec::new();
    walk(p.tree, p.tree, &mut rels)?;
    let mut changed = 0;
    for rel in &rels {
        let src = fsx::safe_join(p.tree, rel)?;
        if rel != crate::schema_check::CONTRACT && base.get(&rel.to_lowercase()).map(|h| h.eq_ignore_ascii_case(&fsx::sha256_file(&src).unwrap_or_default())).unwrap_or(false) {
            continue;
        }
        let dst = fsx::safe_join(&work, rel)?;
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::copy(&src, &dst)?;
        changed += 1;
    }
    progress(&format!("{changed} of {} files differ from the base", rels.len()));
    for f in p.sql {
        let dst = fsx::safe_join(&work, &format!("_migrations/{}/{}.sql", f.db, f.id))?;
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::copy(&f.abs, dst)?;
    }
    let opts = BuildOptions {
        kind: Kind::Update,
        version: p.version.clone(),
        core_commit: p.core_commit.clone(),
        built_at: chrono::Utc::now().to_rfc3339(),
        part_size: p.part_size,
        bots_commit: p.bots_commit.clone(),
        migrations: to_migrations(p.sql),
    };
    let m = package::build(&work, p.out, &opts, &|s| progress(s))?;
    let _ = fs::remove_dir_all(&work);
    Ok(m)
}

/// Sign `manifest.json` in `dir` with the Ed25519 seed (base64) and write `manifest.json.sig`.
pub fn sign_manifest(dir: &Path, seed_b64: &str) -> Result<()> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    let seed: [u8; 32] = STANDARD.decode(seed_b64.trim()).map_err(|_| Error::Invalid("signing key is not base64".into()))?.try_into().map_err(|_| Error::Invalid("signing key has the wrong length".into()))?;
    let bytes = fs::read(dir.join("manifest.json"))?;
    let sig = SigningKey::from_bytes(&seed).sign(&bytes);
    fsx::atomic_write(&dir.join("manifest.json.sig"), format!("{}\n", STANDARD.encode(sig.to_bytes())).as_bytes())?;
    // never publish something the app would reject
    crate::signing::verify_embedded(&bytes, &fs::read_to_string(dir.join("manifest.json.sig"))?)
        .map_err(|_| Error::Invalid("The signature does not match the public key built into the app; wrong signing key.".into()))
}

pub fn revision(commit: &str) -> Revision {
    Revision { commit: Some(commit.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, c: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, c).unwrap();
    }

    #[test]
    fn collects_core_and_module_sql_with_module_base_first_and_stable_ids() {
        let d = tempfile::tempdir().unwrap();
        let c = d.path();
        write(c, "data/sql/updates/db_world/2026_01_01_00.sql", "SELECT 1;");
        write(c, "data/sql/updates/pending_db_world/rev_1.sql", "SELECT 2;");
        write(c, "data/sql/updates/db_characters/2026_01_02_00.sql", "SELECT 3;");
        write(c, "modules/mod-x/data/sql/db-world/base/01_a.sql", "SELECT 4;");
        write(c, "modules/mod-x/data/sql/db-world/2026_02_02_00_b.sql", "SELECT 5;");
        let v = collect_core_sql(c).unwrap();
        let ids: Vec<(&str, &str)> = v.iter().map(|f| (f.db.as_str(), f.id.as_str())).collect();
        assert!(ids.contains(&("world", "2026_01_01_00")) && ids.contains(&("world", "rev_1")) && ids.contains(&("characters", "2026_01_02_00")));
        assert!(ids.contains(&("world", "mod_mod_x__base__01_a")) && ids.contains(&("world", "mod_mod_x__2026_02_02_00_b")));
        // without git history, order falls back to a stable one (all "unknown" keep insertion order)
        let base_pos = ids.iter().position(|x| x.1 == "mod_mod_x__base__01_a").unwrap();
        let upd_pos = ids.iter().position(|x| x.1 == "mod_mod_x__2026_02_02_00_b").unwrap();
        assert!(base_pos < upd_pos, "a module's base script precedes its updates");
    }

    #[test]
    fn cumulative_update_contains_only_what_differs_from_the_base_plus_all_sql() {
        let d = tempfile::tempdir().unwrap();
        let (base_src, base_out, tree, upd_out, sqldir) = (d.path().join("bs"), d.path().join("bo"), d.path().join("tree"), d.path().join("uo"), d.path().join("sql"));
        write(&base_src, "Core/worldserver.exe", "v1");
        write(&base_src, "Core/authserver.exe", "same");
        write(&base_src, crate::schema_check::CONTRACT, "same contract");
        let base = package::build(&base_src, &base_out, &BuildOptions { kind: Kind::Base, version: "1.0.0".into(), core_commit: None, built_at: "x".into(), part_size: 1 << 20, bots_commit: None, migrations: vec![] }, &|_| {}).unwrap();
        write(&tree, "Core/worldserver.exe", "v2");
        write(&tree, "Core/authserver.exe", "same");
        write(&tree, "Core/newlib.dll", "n");
        write(&tree, crate::schema_check::CONTRACT, "same contract");
        write(&sqldir, "world.sql", "SELECT 1;");
        let sql = vec![SqlFile { db: "world".into(), id: "m1".into(), origin: "x".into(), abs: sqldir.join("world.sql"), sha256: fsx::sha256_file(&sqldir.join("world.sql")).unwrap() }];
        let m = pack_update(&UpdateParams { tree: &tree, base_manifest: &base, sql: &sql, out: &upd_out, version: "1.1.0".into(), core_commit: Some("a".repeat(40)), bots_commit: Some("b".repeat(40)), part_size: 1 << 20 }, &|_| {}).unwrap();
        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"Core/worldserver.exe") && paths.contains(&"Core/newlib.dll"));
        assert!(!paths.contains(&"Core/authserver.exe"), "identical to the base: not shipped");
        assert!(paths.contains(&"_migrations/world/m1.sql"));
        assert!(paths.contains(&crate::schema_check::CONTRACT), "the contract ships even when identical to the base");
        assert_eq!(m.kind, Kind::Update);
        assert_eq!(m.migrations.len(), 1);
        assert_eq!(m.bots.as_ref().and_then(|b| b.commit.clone()), Some("b".repeat(40)));
        assert!(!upd_out.with_file_name("uo-staging").exists(), "staging removed");
    }
}
