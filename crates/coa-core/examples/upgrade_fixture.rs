//! Exercise the production updater only on a disposable copy of an installed server.
use coa_core::{registry::MetaDir, update::{self, Env, RepackEnv}};
fn main() -> Result<(),Box<dyn std::error::Error>> {
 let args:Vec<_>=std::env::args().collect();
 if args.len()!=3 {return Err("usage: upgrade_fixture <disposable-root> <signed-package-directory>".into());}
 let root=std::path::Path::new(&args[1]);
 assert!(root.file_name().unwrap().to_string_lossy().starts_with("coa-schema-fixture-real") && root.join(".coa-update-fixture").is_file(),"Disposable fixture required");
 let meta=coa_core::registry::metadata_dir_for(root)?;
 let _=MetaDir::open(&meta)?;
 let env=RepackEnv{root,meta_dir:&meta};
 let outcome=update::apply(&update::Params{root,meta_dir:&meta,source:coa_core::pkgsource::Source::Dir(args[2].clone().into()),trusted_key:coa_core::signing::EMBEDDED_PUBLIC_KEY,cancel:Default::default(),resolutions:Default::default(),env:&env,fail_after_ops:None},&|phase,percent|println!("{percent}% {phase}"));
 let stopped=coa_core::driver::run(root,coa_core::driver::Verb::StopAll);
 println!("Shutdown after test: {:?}",stopped.map(|r|r.ok));
 let outcome=outcome?;
 println!("Final transaction: {:?}",outcome.txn.state);
 env.ensure_stopped()?;
 assert_eq!(outcome.txn.state,update::State::Committed);
 Ok(())
}
