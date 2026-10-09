//! Build and sign a small update package. usage: mkpkg <out dir> <version> <path=content>...
use base64::{engine::general_purpose::STANDARD, Engine};
use coa_core::manifest::Kind;
use coa_core::package::{build, BuildOptions};
use ed25519_dalek::{Signer, SigningKey};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(&a[0]);
    let src = out.with_file_name(format!(
        "{}-src",
        out.file_name().unwrap().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&out);
    for spec in &a[2..] {
        let (p, c) = spec.split_once('=').expect("path=content");
        let f = src.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, c.replace("\\n", "\n")).unwrap();
    }
    build(
        &src,
        &out,
        &BuildOptions {
            kind: Kind::Update,
            version: a[1].clone(),
            core_commit: None,
            built_at: chrono::Utc::now().to_rfc3339(),
            part_size: 1 << 20,
            bots_commit: None,
            migrations: vec![],
        },
        &|_| {},
    )
    .unwrap();
    let key = format!(
        "{}/.coa-manager/signing/manifest-signing.key",
        std::env::var("USERPROFILE").unwrap()
    );
    let seed: [u8; 32] = STANDARD
        .decode(std::fs::read_to_string(key).unwrap().trim())
        .unwrap()
        .try_into()
        .unwrap();
    let sig =
        SigningKey::from_bytes(&seed).sign(&std::fs::read(out.join("manifest.json")).unwrap());
    std::fs::write(
        out.join("manifest.json.sig"),
        STANDARD.encode(sig.to_bytes()),
    )
    .unwrap();
    println!("package {} at {}", a[1], out.display());
}
