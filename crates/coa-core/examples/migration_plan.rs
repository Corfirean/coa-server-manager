//! Read-only audit, or explicit SQL checks on a disposable database fixture.
use coa_core::{
    db::{Account, Db},
    manifest::Manifest,
    migrations::{self, Store},
    realms::Mode,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: migration_plan <root> <signed-manifest> [--apply|--sql-only <migration-directory>]".into());
    }
    let root = std::path::Path::new(&args[1]);
    let bytes = std::fs::read(&args[2])?;
    let signature = std::fs::read_to_string(format!("{}.sig", args[2]))?;
    coa_core::signing::verify(&bytes, &signature, coa_core::signing::EMBEDDED_PUBLIC_KEY)?;
    let manifest = Manifest::parse(&bytes)?;
    for (index, mode) in [Mode::Coa, Mode::Wildcard].into_iter().enumerate() {
        let db = Db::from_repack(root, Account::Admin)?.for_realm(mode);
        let rows = db.load_readonly()?;
        let relevant: Vec<_> = rows
            .iter()
            .filter(|r| r.id == "rev_20260903_01_live_class_baseline")
            .map(|r| format!("{} {:?} {}", r.db, r.status, r.baseline))
            .collect();
        println!("{} baseline history: {:?}", mode.name(), relevant);
        let list: Vec<_> = manifest
            .migrations
            .iter()
            .filter(|m| index == 0 || m.db != "auth")
            .cloned()
            .collect();
        let plan = migrations::status(&db, &list)?;
        println!(
            "{} pending: {:?}",
            mode.name(),
            plan.iter()
                .filter(|m| m.status != migrations::Status::Applied)
                .map(|m| (&m.db, &m.id, m.status))
                .collect::<Vec<_>>()
        );
    }
    if args
        .get(3)
        .is_some_and(|s| s == "--apply" || s == "--sql-only")
    {
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("update-database-diagnosis-")
                && root.join(".coa-update-fixture").is_file(),
            "Only disposable fixture roots may be changed"
        );
        if args[3] == "--sql-only" {
            let db = Db::from_repack(root, Account::Admin)?.for_realm(Mode::Coa);
            let identity = || -> coa_core::Result<String> {
                let mut rows = String::new();
                for (schema, table, columns) in [
                    ("acore_auth", "account", "id,HEX(username)"),
                    ("acore_characters", "characters", "guid,account,HEX(name)"),
                    (
                        "acore_characters_wildcard",
                        "characters",
                        "guid,account,HEX(name)",
                    ),
                ] {
                    rows.push_str(schema);
                    rows.push_str(&db.query(&format!(
                        "SELECT {columns} FROM {schema}.{table} ORDER BY 1;"
                    ))?);
                }
                Ok(coa_core::fsx::sha256_bytes(rows.as_bytes()))
            };
            let before = identity()?;
            migrations::verify_files(&manifest.migrations, std::path::Path::new(&args[4]))?;
            for (index, mode) in [Mode::Coa, Mode::Wildcard].into_iter().enumerate() {
                let realm_db = db.clone().for_realm(mode);
                let list: Vec<_> = manifest
                    .migrations
                    .iter()
                    .filter(|m| index == 0 || m.db != "auth")
                    .cloned()
                    .collect();
                let result = migrations::apply_pending(
                    &realm_db,
                    &list,
                    std::path::Path::new(&args[4]),
                    &|| Ok("disposable fixture".into()),
                )?;
                println!(
                    "{} SQL result: {}",
                    mode.name(),
                    serde_json::to_string(&result)?
                );
                if result.failed.is_some() {
                    return Err("SQL failed; fixture preserved".into());
                }
            }
            assert_eq!(before, identity()?, "Account or character identity changed");
            println!("Account and character identities preserved: {}", before);
            let capture = root.join("captured-schema");
            coa_core::schema_check::capture(&db, &capture)?;
            println!(
                "Captured schema SHA256: {}",
                coa_core::fsx::sha256_file(&capture.join(coa_core::schema_check::CONTRACT))?
            );
            return Ok(());
        }
        if args.len() != 5 {
            return Err("Migration directory required".into());
        }
        use coa_core::update::{Env, RepackEnv};
        let meta = coa_core::registry::metadata_dir_for(root)?;
        let env = RepackEnv {
            root,
            meta_dir: &meta,
        };
        let result = env.migrate(&manifest, std::path::Path::new(&args[4]));
        let _ = coa_core::driver::run(root, coa_core::driver::Verb::StopAll);
        println!("Application result: {}", serde_json::to_string(&result?)?);
    }
    Ok(())
}
