use coa_core::update::{Env, RepackEnv};
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).expect("disposable fixture path"));
    assert_eq!(
        std::fs::read(root.join(".release-schema-fixture")).unwrap(),
        b"disposable release-schema fixture"
    );
    let meta = coa_core::registry::metadata_dir_for(&root).unwrap();
    let env = RepackEnv {
        root: &root,
        meta_dir: &meta,
    };
    let result = env.validate();
    let _ = coa_core::driver::run(&root, coa_core::driver::Verb::StopAll);
    if let Err(error) = result {
        eprintln!("{}", error);
        for log in [
            "Core/Logs/auth-console.log",
            "Core/Logs/world-console.log",
            "Core/Logs/supervisor.log",
        ] {
            eprintln!(
                "{log}:\n{}",
                coa_core::diag::redact(&coa_core::health::tail(&root.join(log), 8192))
            );
        }
        std::process::exit(1);
    }
    println!("Published server startup validation passed on the disposable fixture");
}
