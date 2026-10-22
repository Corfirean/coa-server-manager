//! Sign a manifest: `sign <manifest.json> [key-file]` -> writes <manifest.json>.sig. CI passes the key via COA_SIGNING_KEY.
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use std::fs;

fn main() {
    let mut a = std::env::args().skip(1);
    let manifest = a.next().expect("usage: sign <manifest.json> [key-file]");
    let seed_b64 = match std::env::var("COA_SIGNING_KEY") {
        Ok(v) => v,
        Err(_) => {
            let path = a.next().unwrap_or_else(|| {
                format!(
                    "{}/.coa-manager/signing/manifest-signing.key",
                    std::env::var("USERPROFILE").unwrap()
                )
            });
            fs::read_to_string(path).expect("cannot read signing key")
        }
    };
    let seed: [u8; 32] = STANDARD
        .decode(seed_b64.trim())
        .unwrap()
        .try_into()
        .expect("key length");
    let bytes = fs::read(&manifest).unwrap();
    let sig = SigningKey::from_bytes(&seed).sign(&bytes);
    fs::write(
        format!("{manifest}.sig"),
        format!("{}\n", STANDARD.encode(sig.to_bytes())),
    )
    .unwrap();
    println!("signed {manifest}");
}
