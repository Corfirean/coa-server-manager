//! Stable -> Modules Preview on a DISPOSABLE server: install a stable release, seed a test account and
//! character and a changed module setting, apply a signed preview update with the production updater (which
//! builds the recommended modules and starts the plugin-host core with them), and check that the data and the
//! setting survived.
//!
//! usage: preview_migration_e2e <new folder ending in -fixture> <stable release URL> <preview package dir>
//! The server gets its own free ports before anything starts; it never uses another server's database.
use std::path::Path;

use coa_core::install::{install_base, Params, Source};
use coa_core::registry::{MetaDir, Registry};
use coa_core::update::{self, Env, RepackEnv};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn sql(root: &Path, q: &str) -> String {
    coa_core::backup::with_database(root, |db| db.query(q)).unwrap_or_else(|e| panic!("{q}: {e}"))
}

/// Fingerprint of the seeded rows, read the same way before and after.
fn fingerprint(root: &Path) -> String {
    sql(root, "SELECT CONCAT_WS('|', a.id, a.username, c.guid, c.name, c.level, c.xp, c.money, \
        (SELECT COUNT(*) FROM acore_characters.character_spell s WHERE s.guid = c.guid)) \
        FROM acore_auth.account a JOIN acore_characters.characters c ON c.account = a.id WHERE a.username = 'E2EPREVIEW'")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert_eq!(a.len(), 3, "usage: preview_migration_e2e <new-folder-fixture> <stable release URL> <preview package dir>");
    let root = Path::new(&a[0]);
    assert!(a[0].ends_with("-fixture") && !root.exists(), "a new folder ending in -fixture is required");
    let reg_dir = std::env::temp_dir().join("coa-preview-e2e-registry");
    let _ = std::fs::remove_dir_all(&reg_dir);
    let reg = Registry::at(reg_dir.join("installs.json"));

    // 1. Stable server from its signed installation image.
    let t = std::time::Instant::now();
    let installed = install_base(
        &Params {
            source: Source::Installation(Box::new(Source::Url(a[1].clone()))),
            dest: root.into(),
            trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
            registry: &reg,
            cancel: Default::default(),
        },
        &|s| eprintln!("  {:>3}% {}", s.percent, s.step),
    )?;
    println!("installed stable {} in {:?}", installed.version, t.elapsed());

    // 2. Own ports, before the server is ever started.
    let repack = root.join("Settings/repack.json");
    let mut cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(&repack)?)?;
    for k in ["mysqlPort", "authPort", "worldPort", "raPort"] {
        cfg[k] = free_port().into();
    }
    std::fs::write(&repack, serde_json::to_vec_pretty(&cfg)?)?;
    println!("ports {}", serde_json::json!({"mysql": cfg["mysqlPort"], "auth": cfg["authPort"], "world": cfg["worldPort"], "ra": cfg["raPort"]}));

    // 3. Test data: an account with a character and a spell; a changed module setting.
    let cols = sql(root, "SELECT GROUP_CONCAT(CONCAT(COLUMN_NAME, ':', DATA_TYPE)) FROM information_schema.COLUMNS \
        WHERE TABLE_SCHEMA='acore_characters' AND TABLE_NAME='characters' AND IS_NULLABLE='NO' AND COLUMN_DEFAULT IS NULL");
    sql(root, "INSERT INTO acore_auth.account (username, salt, verifier, email, reg_mail) \
        VALUES ('E2EPREVIEW', UNHEX(REPEAT('11',32)), UNHEX(REPEAT('22',32)), '', '')");
    let account = sql(root, "SELECT id FROM acore_auth.account WHERE username='E2EPREVIEW'").trim().to_string();
    let mut names = vec!["guid", "account", "name", "race", "class", "gender", "level", "xp", "money"].into_iter().map(String::from).collect::<Vec<_>>();
    let mut values = vec!["990001".to_string(), account, "'Eeprev'".into(), "1".into(), "1".into(), "0".into(), "42".into(), "777".into(), "123456".into()];
    for c in cols.trim().split(',').filter(|c| !c.is_empty()) {
        let (name, ty) = c.split_once(':').unwrap();
        if names.iter().any(|n| n == name) {
            continue;
        }
        names.push(name.to_string());
        values.push(if ty.contains("char") || ty.contains("text") || ty.contains("blob") { "''".into() } else { "0".into() });
    }
    sql(root, &format!("INSERT INTO acore_characters.characters ({}) VALUES ({})", names.join(","), values.join(",")));
    sql(root, "INSERT INTO acore_characters.character_spell (guid, spell, specMask) VALUES (990001, 133, 255)");
    let conf = root.join("Core/configs/modules/mod-coa-content-scaling.conf");
    let text = std::fs::read_to_string(&conf)?;
    let line = text.lines().find(|l| !l.trim_start().starts_with('#') && l.contains(" = ")).expect("a setting").to_string();
    let changed = format!("{} = 7", line.split(" = ").next().unwrap().trim());
    std::fs::write(&conf, text.replacen(&line, &changed, 1))?;
    let before = fingerprint(root);
    println!("before: {before}\nsetting: {changed}");
    assert!(!before.trim().is_empty());

    // 4. The preview update, through the production updater.
    let meta = coa_core::registry::metadata_dir_for(root)?;
    let (_, info) = MetaDir::open(&meta)?;
    assert_eq!(coa_core::fsx::canonicalize_lenient(Path::new(&info.server_path))?, coa_core::fsx::canonicalize_lenient(root)?);
    let env = RepackEnv { root, meta_dir: &meta };
    let t = std::time::Instant::now();
    let outcome = update::apply(
        &update::Params {
            root,
            meta_dir: &meta,
            source: Source::Dir(a[2].clone().into()),
            trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
            cancel: Default::default(),
            resolutions: Default::default(),
            env: &env,
            fail_after_ops: None,
        },
        &|phase, percent| println!("{percent}% {phase}"),
    );
    let stopped = coa_core::driver::run(root, coa_core::driver::Verb::StopAll);
    println!("shutdown after update: {:?}", stopped.map(|r| r.ok));
    let outcome = outcome?;
    println!("update {:?} in {:?}; modules {:#?}", outcome.txn.state, t.elapsed(), outcome.modules);

    // 5. Same data, same setting, plugin host with its modules.
    let after = fingerprint(root);
    let text = std::fs::read_to_string(&conf)?;
    println!("after:  {after}");
    println!("setting kept: {}", text.lines().any(|l| l.trim() == changed));
    for p in ["Core/plugins/plugins.lock", "Core/plugins/plugin-boot-report.json"] {
        if let Ok(t) = std::fs::read_to_string(root.join(p)) {
            println!("--- {p}\n{t}");
        }
    }
    env.ensure_stopped()?;
    assert_eq!(outcome.txn.state, update::State::Committed);
    assert_eq!(before, after, "seeded character data changed");
    assert!(text.lines().any(|l| l.trim() == changed), "module setting lost");
    println!("PASS");
    Ok(())
}
