use coa_core::{backup, schema_check, Result};
fn main()->Result<()> {
    let root=std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let contract=std::path::PathBuf::from(std::env::args().nth(2).unwrap());
    assert!(root.join("recovery-e2e-started").is_file());
    assert!(contract.join(".release-schema-fixture").is_file());
    backup::with_database(&root,|db| {for p in schema_check::check(db,&contract)? {println!("{}.{}.{}: {}",p.database,p.table,p.column,p.detail);} Ok(())})
}
