//! Live check of the client download path against the community launcher's public server:
//! reads the manifest and downloads the few smallest files into a temp folder (never a real client).
//! Usage: cargo run -p coa-core --example client_probe   (PROBE_SKIP=n skips the n smallest files)

use coa_core::clientdl::{self, Apply, Manifest};
use coa_core::download::{Cancel, HttpTransport};

fn main() {
    let full = clientdl::fetch_latest().expect("manifest");
    println!("version {} · {} files · {:.1} GB", full.version, full.files.len(), full.total_bytes() as f64 / 1e9);
    let mut small = full.files.clone();
    small.sort_by_key(|f| f.size);
    let skip: usize = std::env::var("PROBE_SKIP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let small: Vec<_> = small.into_iter().skip(skip).take(6).collect();
    let manifest = Manifest { version: full.version.clone(), published_at: None, files: small };

    let dir = std::env::temp_dir().join(format!("coa-client-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cancel = Cancel::default();
    let plan = clientdl::plan(&dir, &manifest, &Default::default(), &cancel, &|_| {}).unwrap();
    println!("plan: {} files, {} bytes", plan.items.len(), plan.download_bytes);
    let transport = HttpTransport::new().unwrap();
    clientdl::apply(
        &Apply { client: &dir, manifest: &manifest, plan: &plan, keep_modified: false, transport: &transport, objects_url: clientdl::OBJECTS_URL, cancel: &cancel },
        &|s| println!("{} {}/{} {}", s.phase, s.done, s.total, s.file.unwrap_or_default()),
    )
    .expect("apply");
    let again = clientdl::plan(&dir, &manifest, &clientdl::load_state(&dir).unwrap(), &cancel, &|_| {}).unwrap();
    println!("second plan: {} to do, {} up to date, version {:?}", again.items.len(), again.up_to_date_files, clientdl::local(&dir).version);
    std::fs::remove_dir_all(&dir).unwrap();
}
