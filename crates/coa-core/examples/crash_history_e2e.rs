use coa_core::{crashes, Result};
fn main() -> Result<()> {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("native crash evidence"));
    assert!(root.join(".crash-report-evidence").is_file());
    let reports = crashes::list(&root, 10)?;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].exception_code.as_deref(), Some("C0000005"));
    assert_eq!(reports[0].category, "access_violation");
    assert!(!reports[0].full_log.is_empty());
    println!("PASS: Manager crash history discovers and classifies the actual native handler report from Core/Crashes");
    Ok(())
}
