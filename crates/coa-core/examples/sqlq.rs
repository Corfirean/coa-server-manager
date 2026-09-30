//! Read-only helper for a DISPOSABLE fixture: run one query. usage: sqlq <fixture> <sql>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    assert!(a[0].to_lowercase().contains("fixture"), "refusing: not a fixture folder");
    let root = std::path::Path::new(&a[0]);
    let out = coa_core::backup::with_database(root, |db| db.query(&a[1])).expect("query");
    println!("{out}");
}
