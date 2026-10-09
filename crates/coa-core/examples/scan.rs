use std::path::Path;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: scan <server folder>");
    let root = Path::new(&path);
    let report = coa_core::layout::scan(root).expect("scan failed");
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    let observed = coa_core::process::observe(root, &report.ports);
    println!("{}", serde_json::to_string_pretty(&observed).unwrap());
}
