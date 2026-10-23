use std::path::Path;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = args
        .next()
        .expect("usage: drive <server folder> <start|stop>");
    let verb = match args.next().as_deref() {
        Some("start") => coa_core::driver::Verb::StartAll,
        Some("mysql") => coa_core::driver::Verb::StartMysql,
        Some("stop") => coa_core::driver::Verb::StopAll,
        _ => panic!("verb must be start or stop"),
    };
    let outcome = coa_core::driver::run(Path::new(&root), verb).expect("driver failed");
    println!("{}", serde_json::to_string_pretty(&outcome).unwrap());
    let report = coa_core::layout::scan(Path::new(&root)).unwrap();
    let observed = coa_core::process::observe(Path::new(&root), &report.ports);
    println!("{}", serde_json::to_string_pretty(&observed).unwrap());
}
