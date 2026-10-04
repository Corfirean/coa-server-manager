//! Release tooling for the CoA server packages. Used by CI and by the maintainer; never shipped to players.
//!
//!   coa-release pack-base   --tree DIR --out DIR --version X [--core-commit SHA] [--part-size BYTES]
//!   coa-release pack-update --tree DIR --base-manifest FILE --core DIR --out DIR --version X
//!                           [--bots DIR] [--core-commit SHA] [--bots-commit SHA] [--part-size BYTES]
//!   coa-release clean-base  --repack DIR --core DIR --tree DIR --out DIR [--bots DIR] [--data DIR]
//!   coa-release sign        --dir DIR        (key: env COA_SIGNING_KEY, or ~/.coa-manager/signing/manifest-signing.key)
//!   coa-release verify      --dir DIR        (against the public key built into this tool)

use std::collections::HashMap;
use std::path::PathBuf;

use coa_core::manifest::{Kind, Manifest};
use coa_core::package::{build, BuildOptions, DEFAULT_PART_SIZE};
use coa_core::release::{collect_bots_sql, collect_core_sql, pack_update, sign_manifest, UpdateParams};

fn args(rest: &[String]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut it = rest.iter();
    while let Some(k) = it.next() {
        if let (Some(k), Some(v)) = (k.strip_prefix("--"), it.next()) {
            m.insert(k.to_string(), v.clone());
        }
    }
    m
}

fn need<'a>(a: &'a HashMap<String, String>, k: &str) -> Result<&'a String, String> {
    a.get(k).ok_or_else(|| format!("missing --{k}"))
}

fn signing_key() -> Result<String, String> {
    if let Ok(k) = std::env::var("COA_SIGNING_KEY") {
        return Ok(k);
    }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).map_err(|_| "no home folder".to_string())?;
    std::fs::read_to_string(PathBuf::from(home).join(".coa-manager/signing/manifest-signing.key")).map_err(|_| "no signing key: set COA_SIGNING_KEY".to_string())
}

fn run() -> Result<(), String> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = all.split_first().ok_or("usage: coa-release <pack-base|pack-update|sign|verify> ...")?;
    let a = args(rest);
    let e = |x: coa_core::Error| x.to_string();
    let part = a.get("part-size").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_PART_SIZE);
    match cmd.as_str() {
        "schema-contract" => {
            let repack = PathBuf::from(need(&a, "repack")?);
            let tree = PathBuf::from(need(&a, "tree")?);
            coa_core::backup::with_database(&repack, |db| coa_core::schema_check::capture(db, &tree)).map_err(e)?;
            println!("database schema contract captured");
        }
        "pack-base" => {
            let opts = BuildOptions { kind: Kind::Base, version: need(&a, "version")?.clone(), core_commit: a.get("core-commit").cloned(), built_at: chrono_now(), part_size: part, bots_commit: a.get("bots-commit").cloned(), migrations: vec![] };
            let m = build(&PathBuf::from(need(&a, "tree")?), &PathBuf::from(need(&a, "out")?), &opts, &|s| eprintln!("{s}")).map_err(e)?;
            println!("base {}: {} files, {} parts", m.version, m.files.len(), m.archive.as_ref().map(|x| x.parts.len()).unwrap_or(0));
        }
        "pack-update" => {
            let base: Manifest = Manifest::parse(&std::fs::read(need(&a, "base-manifest")?).map_err(|x| x.to_string())?).map_err(e)?;
            let mut sql = collect_core_sql(&PathBuf::from(need(&a, "core")?)).map_err(e)?;
            if let Some(b) = a.get("bots") {
                sql.extend(collect_bots_sql(&PathBuf::from(b)).map_err(e)?);
            }
            let out = PathBuf::from(need(&a, "out")?);
            let m = pack_update(
                &UpdateParams { tree: &PathBuf::from(need(&a, "tree")?), base_manifest: &base, sql: &sql, out: &out, version: need(&a, "version")?.clone(), core_commit: a.get("core-commit").cloned(), bots_commit: a.get("bots-commit").cloned(), part_size: part },
                &|s| eprintln!("{s}"),
            )
            .map_err(e)?;
            println!("update {}: {} files, {} migrations", m.version, m.files.len(), m.migrations.len());
        }
        "clean-base" => {
            let (repack, core, tree, out) = (PathBuf::from(need(&a, "repack")?), PathBuf::from(need(&a, "core")?), PathBuf::from(need(&a, "tree")?), PathBuf::from(need(&a, "out")?));
            let bots = a.get("bots").map(PathBuf::from);
            let data = a.get("data").map(PathBuf::from);
            coa_core::cleanbase::build(&coa_core::cleanbase::Params { repack: &repack, core: &core, bots: bots.as_deref(), tree: &tree, data: data.as_deref(), out: &out }, &|s| eprintln!("{s}")).map_err(e)?;
            println!("clean base tree at {}", out.display());
        }
        "sign" => {
            let dir = PathBuf::from(need(&a, "dir")?);
            sign_manifest(&dir, &signing_key()?).map_err(e)?;
            println!("signed {}", dir.join("manifest.json").display());
        }
        "verify" => {
            let dir = PathBuf::from(need(&a, "dir")?);
            let bytes = std::fs::read(dir.join("manifest.json")).map_err(|x| x.to_string())?;
            let sig = std::fs::read_to_string(dir.join("manifest.json.sig")).map_err(|x| x.to_string())?;
            coa_core::signing::verify_embedded(&bytes, &sig).map_err(e)?;
            Manifest::parse(&bytes).map_err(e)?;
            println!("signature and manifest OK");
        }
        other => return Err(format!("unknown command {other}")),
    }
    Ok(())
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
