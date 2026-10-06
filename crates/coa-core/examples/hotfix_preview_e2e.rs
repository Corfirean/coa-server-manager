//! Read-only update preview against the marked disposable acceptance fixture.
use std::{collections::BTreeMap,path::PathBuf};
fn main()->coa_core::Result<()> {
    let args:Vec<_>=std::env::args().skip(1).collect();
    let root=PathBuf::from(&args[0]);
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("coa-schema-fixture-chaos-064-transition-"));
    assert_eq!(std::fs::read(root.join(".transition-fixture"))?,b"disposable published-064 transition fixture");
    let meta_dir=coa_core::registry::metadata_dir_for(&root)?;
    let (_,meta)=coa_core::registry::MetaDir::open(&meta_dir)?;
    let preview=coa_core::update::preview(&root,&meta,&coa_core::pkgsource::Source::Dir(args[1].clone().into()),coa_core::signing::EMBEDDED_PUBLIC_KEY,&BTreeMap::new())?;
    assert_eq!(preview.from_version.as_deref(),Some(preview.to_version.as_str()));
    assert_eq!(preview.pending_migrations,0);
    assert!(preview.items.iter().all(|item|item.action==coa_core::update::Action::Skip),"{:?}",preview.items.iter().filter(|item|item.action!=coa_core::update::Action::Skip).collect::<Vec<_>>());
    println!("PASS: real fixture repeated preview: {} files unchanged, zero pending SQL, no update offered",preview.items.len());
    Ok(())
}
