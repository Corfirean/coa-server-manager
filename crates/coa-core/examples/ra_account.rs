//! Create an account through the Manager's RA client on a RUNNING disposable fixture.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a[0].to_lowercase().contains("fixture") || a[0].to_lowercase().contains("installtest") || a[0].to_lowercase().contains("cleaninstall"), "refusing: not a disposable folder");
    let root = std::path::Path::new(&a[0]);
    let mut ra = coa_core::ra::Ra::connect(root).expect("connect");
    ra.create_account(&a[1], &a[2]).expect("create");
    ra.make_administrator(&a[1]).expect("gm");
    println!("created {} (administrator)", a[1]);
    let out = coa_core::backup::with_database(root, |db| db.query(&format!("SELECT a.username, aa.gmlevel FROM acore_auth.account a LEFT JOIN acore_auth.account_access aa ON aa.id=a.id WHERE a.username='{}';", a[1].to_uppercase()))).unwrap();
    println!("in database: {out}");
}
