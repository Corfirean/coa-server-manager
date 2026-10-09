fn main() {
    let p = std::env::args().nth(1).expect("client folder");
    println!(
        "{}",
        serde_json::to_string_pretty(&coa_core::client::detect(std::path::Path::new(&p), None))
            .unwrap()
    );
}
