//! Check and apply a real published update on a DISPOSABLE install. usage: update_url <install folder> <update url>
//! The folder name must contain "installtest"; ports must already point away from any running server.
use coa_core::download::Cancel;
use coa_core::pkgsource::Source;
use coa_core::registry::{metadata_dir_for, MetaDir};
use coa_core::update::{apply, preview, Params, RepackEnv};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::PathBuf::from(&a[0]);
    assert!(a[0].to_lowercase().contains("installtest"), "refusing: not a disposable test install");
    let dir = metadata_dir_for(&root).unwrap();
    let (_, meta) = MetaDir::open(&dir).unwrap();
    let src = Source::Url(a[1].clone());
    let p = preview(&root, &meta, &src, coa_core::signing::EMBEDDED_PUBLIC_KEY, &Default::default()).unwrap_or_else(|e| panic!("preview failed: {e:?}"));
    println!("preview: {} -> {} | download {} bytes | {} files change | {} migrations | conflicts {:?}", p.from_version.clone().unwrap_or_default(), p.to_version, p.download_bytes, p.items.iter().filter(|i| format!("{:?}", i.action) != "Skip").count(), p.migrations, p.conflicts);
    let env = RepackEnv { root: &root, meta_dir: &dir };
    let t = std::time::Instant::now();
    let out = apply(
        &Params { root: &root, meta_dir: &dir, source: src, trusted_key: coa_core::signing::EMBEDDED_PUBLIC_KEY, cancel: Cancel::default(), resolutions: Default::default(), env: &env, fail_after_ops: None },
        &|s, pct| eprintln!("  {pct:>3}% {s}"),
    )
    .unwrap_or_else(|e| panic!("apply failed: {e:?}"));
    println!("apply finished in {:?}: state {:?}, to {:?}", t.elapsed(), out.txn.state, out.txn.to_version);
}
