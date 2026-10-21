//! Release tooling for the CoA server packages. Used by CI and by the maintainer; never shipped to players.
//!
//!   coa-release pack-base   --tree DIR --out DIR --version X [--core-commit SHA] [--part-size BYTES]
//!   coa-release pack-update --tree DIR --base-manifest FILE --core DIR --out DIR --version X
//!                           [--bots DIR] [--repairs DIR] [--core-commit SHA] [--bots-commit SHA] [--part-size BYTES]
//!   coa-release extract-schema-base --package DIR --fixture NEW-coa-schema-fixture-DIR
//!   coa-release schema-contract --repack FIXTURE --tree DIR --core DIR [--bots DIR] [--repairs DIR]
//!   coa-release clean-base  --repack DIR --core DIR --tree DIR --out DIR [--bots DIR] [--data DIR]
//!   coa-release sign        --dir DIR        (key: env COA_SIGNING_KEY, or ~/.coa-manager/signing/manifest-signing.key)
//!   coa-release verify      --dir DIR        (against the public key built into this tool)

use std::collections::HashMap;
use std::path::PathBuf;

use coa_core::manifest::{Kind, Manifest};
use coa_core::package::{build, BuildOptions, DEFAULT_PART_SIZE};
use coa_core::release::{pack_update, sign_manifest, UpdateParams};

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
        "verify-registry" => {
            let sql = coa_core::migration_registry::reconcile(&PathBuf::from(need(&a, "registry")?), Vec::new()).map_err(e)?;
            println!("Verified {} historical migrations, order and signed byte variants", sql.len());
        }
        "resolve" => {
            let package = coa_core::pkgsource::fetch_package(&coa_core::pkgsource::Source::Url(need(&a, "source")?.clone()), coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            coa_core::fsx::atomic_write(&PathBuf::from(need(&a, "out")?), &package.bytes).map_err(e)?;
        }
        "extract-game-data" => {
            let source = PathBuf::from(need(&a, "package")?);
            let package = coa_core::pkgsource::fetch_package(&coa_core::pkgsource::Source::Dir(source.clone()), coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            if !package.manifest.files.iter().any(|f| f.path.starts_with("Data/")) { return Err("Signed base has no game data.".into()); }
            coa_core::package::extract_selected(&source, &package.manifest, &PathBuf::from(need(&a, "fixture")?), &|p| p.starts_with("Data/"), &|_, _| {}).map_err(e)?;
        }
        "repackage" => {
            let source = PathBuf::from(need(&a, "package")?);
            let pinned = coa_core::pkgsource::fetch_package(&coa_core::pkgsource::Source::Dir(source.clone()), coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            let work = tempfile::tempdir().map_err(|e| e.to_string())?;
            coa_core::package::extract(&source, &pinned.manifest, work.path(), &|_, _| {}).map_err(e)?;
            let mut sql: Vec<_> = pinned.manifest.migrations.iter().map(|m| coa_core::release::SqlFile {
                compatible_sha256: m.compatible_sha256.clone(), db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), origin: "signed package".into(), abs: work.path().join("_migrations").join(&m.db).join(format!("{}.sql", m.id)),
            }).collect();
            let repairs = PathBuf::from(need(&a, "repairs")?);
            for repair in coa_core::release_schema::collect(&work.path().join("no-core"), None, Some(&repairs)).map_err(e)? {
                if !sql.iter().any(|m| m.db == repair.db && m.id == repair.id) { sql.push(repair); }
            }
            let sql = coa_core::migration_registry::reconcile(&PathBuf::from(need(&a, "registry")?), sql).map_err(e)?;
            let migrations = sql.iter().map(|m| coa_core::manifest::Migration { compatible_sha256: m.compatible_sha256.clone(), db: m.db.clone(), id: m.id.clone(), sha256: m.sha256.clone(), destructive: false }).collect();
            for file in &sql { coa_core::fsx::atomic_write(&work.path().join("_migrations").join(&file.db).join(format!("{}.sql", file.id)), &std::fs::read(&file.abs).map_err(|e| e.to_string())?).map_err(e)?; }
            let out = PathBuf::from(need(&a, "out")?);
            let opts = BuildOptions { kind: Kind::Update, version: need(&a, "version")?.clone(), core_commit: pinned.manifest.core.commit, bots_commit: pinned.manifest.bots.and_then(|b| b.commit), built_at: chrono_now(), part_size: part, migrations };
            let mut manifest = build(work.path(), &out, &opts, &|s| eprintln!("{s}")).map_err(e)?;
            manifest.min_manager_version = coa_core::MANAGER_VERSION.into();
            coa_core::fsx::atomic_write_json(&out.join("manifest.json"), &manifest).map_err(e)?;
        }
        "archive-migrations" => {
            coa_core::migration_registry::import(&PathBuf::from(need(&a, "package")?), &PathBuf::from(need(&a, "registry")?), coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            println!("Published migration bytes and compatibility archived");
        }
        "verify-channel" => {
            let pointer = coa_core::pkgsource::verify_channel(&std::fs::read(need(&a, "file")?).map_err(|e| e.to_string())?, coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            let dir = PathBuf::from(need(&a, "dir")?);
            let manifest: Manifest = coa_core::fsx::read_json(&dir.join("manifest.json")).map_err(e)?;
            if manifest.version != pointer.version || coa_core::fsx::sha256_file(&dir.join("manifest.json")).map_err(e)? != pointer.manifest_sha256 || pointer.acceptance_sha256.as_deref() != Some(&coa_core::fsx::sha256_file(&dir.join("acceptance.json")).map_err(e)?) {
                return Err("Signed channel differs from the tested package/report".into());
            }
        }
        "channel" => {
            use base64::{engine::general_purpose::STANDARD, Engine};
            use ed25519_dalek::{Signer, SigningKey};
            let dir = PathBuf::from(need(&a, "dir")?);
            let pinned = coa_core::pkgsource::fetch_package(&coa_core::pkgsource::Source::Dir(dir.clone()), coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            let pointer = coa_core::pkgsource::Channel { schema: 1, version: pinned.manifest.version.clone(), release: format!("https://github.com/Corfirean/coa-server-build/releases/download/server-{}/", pinned.manifest.version), manifest_sha256: coa_core::fsx::sha256_bytes(&pinned.bytes), acceptance_sha256: Some(coa_core::fsx::sha256_file(&dir.join("acceptance.json")).map_err(e)?) };
            let payload = serde_json::to_vec(&pointer).map_err(|e| e.to_string())?;
            let seed: [u8; 32] = STANDARD.decode(signing_key()?.trim()).map_err(|e| e.to_string())?.try_into().map_err(|_| "Wrong signing key length")?;
            let signed = coa_core::pkgsource::SignedChannel { payload: STANDARD.encode(&payload), signature: STANDARD.encode(SigningKey::from_bytes(&seed).sign(&payload).to_bytes()) };
            coa_core::signing::verify_embedded(&payload, &signed.signature).map_err(e)?;
            coa_core::fsx::atomic_write_json(&PathBuf::from(need(&a, "out")?), &signed).map_err(e)?;
        }
        "extract-schema-base" => {
            coa_core::release_schema::extract_base(
                &PathBuf::from(need(&a, "package")?), &PathBuf::from(need(&a, "fixture")?),
                coa_core::signing::EMBEDDED_PUBLIC_KEY,
            ).map_err(e)?;
            println!("signed base extracted into a disposable database fixture");
        }
        "schema-contract" => {
            let repack = PathBuf::from(need(&a, "repack")?);
            let tree = PathBuf::from(need(&a, "tree")?);
            if let Some(core) = a.get("core") {
                let bots = a.get("bots").map(PathBuf::from);
                let repairs = a.get("repairs").map(PathBuf::from);
                let mut sql = coa_core::release_schema::collect(&PathBuf::from(core), bots.as_deref(), repairs.as_deref()).map_err(e)?;
                if let Some(registry) = a.get("registry") { sql = coa_core::migration_registry::reconcile(&PathBuf::from(registry), sql).map_err(e)?; }
                coa_core::release_schema::capture_release(&repack, &tree, &sql).map_err(e)?;
            } else {
                coa_core::backup::with_database(&repack, |db| coa_core::schema_check::capture(db, &tree)).map_err(e)?;
            }
            println!("database schema contract captured");
        }
        "pack-base" => {
            coa_core::schema_check::require_release_contract(&PathBuf::from(need(&a, "tree")?)).map_err(e)?;
            let opts = BuildOptions { kind: Kind::Base, version: need(&a, "version")?.clone(), core_commit: a.get("core-commit").cloned(), built_at: chrono_now(), part_size: part, bots_commit: a.get("bots-commit").cloned(), migrations: vec![] };
            let m = build(&PathBuf::from(need(&a, "tree")?), &PathBuf::from(need(&a, "out")?), &opts, &|s| eprintln!("{s}")).map_err(e)?;
            println!("base {}: {} files, {} parts", m.version, m.files.len(), m.archive.as_ref().map(|x| x.parts.len()).unwrap_or(0));
        }
        "pack-update" => {
            let base: Manifest = Manifest::parse(&std::fs::read(need(&a, "base-manifest")?).map_err(|x| x.to_string())?).map_err(e)?;
            coa_core::schema_check::require_release_contract(&PathBuf::from(need(&a, "tree")?)).map_err(e)?;
            let bots = a.get("bots").map(PathBuf::from);
            let repairs = a.get("repairs").map(PathBuf::from);
            let mut sql = coa_core::release_schema::collect(&PathBuf::from(need(&a, "core")?), bots.as_deref(), repairs.as_deref()).map_err(e)?;
            if let Some(registry) = a.get("registry") { sql = coa_core::migration_registry::reconcile(&PathBuf::from(registry), sql).map_err(e)?; }
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
            require_contract_entry(&dir)?;
            sign_manifest(&dir, &signing_key()?).map_err(e)?;
            coa_core::release_schema::verify_package(&dir, coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            println!("signed {}", dir.join("manifest.json").display());
        }
        "verify" => {
            let dir = PathBuf::from(need(&a, "dir")?);
            coa_core::release_schema::verify_package(&dir, coa_core::signing::EMBEDDED_PUBLIC_KEY).map_err(e)?;
            println!("signature, archive contents and database schema contract OK");
        }
        other => return Err(format!("unknown command {other}")),
    }
    Ok(())
}

fn require_contract_entry(dir: &std::path::Path) -> Result<(), String> {
    let manifest = Manifest::parse(&std::fs::read(dir.join("manifest.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if !manifest.files.iter().any(|f| f.path == coa_core::schema_check::CONTRACT && f.size > 0) {
        return Err("The release is missing its database schema contract; refusing to sign or publish it.".into());
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
