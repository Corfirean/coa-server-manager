fn main() {
    let m = std::env::args()
        .nth(1)
        .expect("usage: verify <manifest.json>");
    let bytes = std::fs::read(&m).unwrap();
    let sig = std::fs::read_to_string(format!("{m}.sig")).unwrap();
    match coa_core::signing::verify_embedded(&bytes, &sig) {
        Ok(()) => println!("signature OK"),
        Err(e) => {
            println!("FAILED: {e}");
            std::process::exit(1)
        }
    }
}
