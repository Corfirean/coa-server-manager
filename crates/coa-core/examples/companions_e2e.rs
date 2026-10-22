//! Adds the template characters to a scratch install and creates companions offline. usage: companions_e2e <install folder> <count>
//! The folder name must contain "installtest" and its database must already be running (Scripts/manage.py start-mysql).
use coa_core::db::{Account, Db};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::PathBuf::from(&a[0]);
    assert!(
        a[0].to_lowercase().contains("installtest"),
        "refusing to touch a server that is not a disposable test install"
    );
    let count: u32 = a[1].parse().unwrap();
    let db = Db::from_repack(&root, Account::Admin).expect("db");
    println!(
        "templates before: {}",
        coa_core::companions::template_count(&db).unwrap()
    );
    println!(
        "added: {}",
        coa_core::companions::ensure_templates(&db).unwrap()
    );
    println!(
        "templates after: {}",
        coa_core::companions::template_count(&db).unwrap()
    );
    let t = std::time::Instant::now();
    match coa_core::companions::offline_create(&root, &root.join("companions-e2e.log"), count) {
        Ok(tail) => println!("offline create ok in {:?}:\n{tail}", t.elapsed()),
        Err(e) => println!("offline create FAILED: {e}"),
    }
}
