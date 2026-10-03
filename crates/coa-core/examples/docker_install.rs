//! Install a new Docker server from a package folder, for a manual end-to-end check on Linux (needs docker).
//! usage: docker_install <package folder> <destination> <game data folder> <public key (base64)>
//! The key is the one the package was signed with (the production key is only used by the Manager itself).
use coa_core::docker::install::{install, Params};
use coa_core::download::Cancel;
use coa_core::install::Source;
use coa_core::registry::Registry;
use std::path::PathBuf;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a.len() == 4, "usage: docker_install <package folder> <destination> <game data folder> <public key>");
    let registry = Registry::at(std::env::temp_dir().join("coa-docker-install/installs.json"));
    let t = std::time::Instant::now();
    let done = install(
        &Params { source: Source::Dir(PathBuf::from(&a[0])), dest: PathBuf::from(&a[1]), data_dir: PathBuf::from(&a[2]), trusted_key: &a[3], registry: &registry, cancel: Cancel::default() },
        &|s| eprintln!("  {:>3}% {} {}", s.percent, s.step, s.detail.unwrap_or_default()),
    )
    .unwrap_or_else(|e| panic!("install failed: {e}"));
    println!("installed {} at {} in {:?}", done.version, done.path, t.elapsed());
}
