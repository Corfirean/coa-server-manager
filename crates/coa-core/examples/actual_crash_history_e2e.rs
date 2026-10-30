use coa_core::{crashes, Result};
fn main()->Result<()> {
    let root=std::path::PathBuf::from(std::env::args().nth(1).expect("marked native fixture"));
    assert!(root.join("recovery-e2e-started").is_file());
    let reports=crashes::list(&root,30)?;
    let matching:Vec<_>=reports.iter().filter(|r|r.full_log.contains("AllowableRaces")).collect();
    assert!(!matching.is_empty());
    assert!(matching.iter().all(|r|r.exception_code.as_deref()==Some("C0000420")));
    println!("PASS: Manager discovered {} actual worldserver SQL assertion reports from Core/Crashes, including Windows ANSI text",matching.len());
    Ok(())
}
