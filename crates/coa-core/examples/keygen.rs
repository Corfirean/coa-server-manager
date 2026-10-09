//! One-time release-key generation. Writes the private seed to a user-only folder OUTSIDE the repository
//! and the public key to keys/manifest-signing.pub. Refuses to overwrite an existing key.
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::SigningKey;
use std::{fs, path::PathBuf};

fn main() {
    let private = PathBuf::from(std::env::var("USERPROFILE").expect("USERPROFILE"))
        .join(".coa-manager/signing/manifest-signing.key");
    let public = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "keys/manifest-signing.pub".into()),
    );
    if private.exists() || public.exists() {
        panic!("a key already exists; refusing to overwrite");
    }
    fs::create_dir_all(private.parent().unwrap()).unwrap();
    let sk = SigningKey::generate(&mut rand_core::OsRng);
    fs::write(&private, STANDARD.encode(sk.to_bytes())).unwrap();
    fs::write(
        &public,
        format!("{}\n", STANDARD.encode(sk.verifying_key().to_bytes())),
    )
    .unwrap();
    println!(
        "private key: {}\npublic key:  {}",
        private.display(),
        public.display()
    );
}
