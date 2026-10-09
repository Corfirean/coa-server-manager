//! Install a signed package folder into a NEW folder (production trust key). usage: install_pkg <package dir> <destination>
use coa_core::download::Cancel;
use coa_core::install::{install_base, Params, Source};
use coa_core::registry::Registry;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let reg = Registry::at(
        std::env::temp_dir()
            .join("coa-install-pkg")
            .join("installs.json"),
    );
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("coa-install-pkg"));
    let t = std::time::Instant::now();
    let r = install_base(
        &Params {
            source: Source::Dir(a[0].clone().into()),
            dest: a[1].clone().into(),
            trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY,
            registry: &reg,
            cancel: Cancel::default(),
        },
        &|s| eprintln!("  {:>3}% {}", s.percent, s.step),
    )
    .unwrap_or_else(|e| panic!("install failed: {e:?}"));
    println!("installed {} at {} in {:?}", r.version, r.path, t.elapsed());
}
