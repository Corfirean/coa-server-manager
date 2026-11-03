use coa_core::{driver, fsx, process, Result};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
};

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else {
            std::fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}
fn spawn(path: &Path) -> OwnedChild {
    let mut cmd = Command::new(path);
    cmd.args(["-t", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    OwnedChild(cmd.spawn().unwrap())
}
fn main() -> Result<()> {
    assert!(cfg!(windows));
    let source = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("marked disposable runtime source"),
    );
    assert_eq!(
        std::fs::read(source.join(".release-schema-fixture"))?,
        b"disposable release-schema fixture"
    );
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    copy_tree(&source.join("Runtime"), &root.join("Runtime"))?;
    std::fs::create_dir_all(root.join("Core"))?;
    std::fs::create_dir_all(root.join("Scripts"))?;
    let system =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/ping.exe");
    std::fs::copy(&system, root.join("Core/worldserver.exe"))?;
    std::fs::copy(&system, root.join("foreign.exe"))?;
    let mut ports = Vec::new();
    let mut listeners = Vec::new();
    for _ in 0..4 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        ports.push(listener.local_addr()?.port());
        listeners.push(listener);
    }
    let config = serde_json::json!({"mysqlPort":ports[0],"authPort":ports[1],"worldPort":ports[2],"raPort":ports[3],"raUsername":"fixture","raPassword":"fixture"});
    fsx::atomic_write_json(&root.join("Settings/repack.json"), &config)?;
    drop(listeners);
    let mut launcher = std::fs::read_to_string(source.join("Scripts/manage.py"))?;
    launcher.push_str("\nfrom contextlib import nullcontext\ndef control_lock(): return nullcontext()\ndef stop_all(config):\n    assert not process('world'), 'world must stop before database'\n    (ROOT/'database-stage').write_text('safe')\n");
    fsx::atomic_write(&root.join("Scripts/manage.py"), launcher.as_bytes())?;
    let mut foreign = spawn(&root.join("foreign.exe"));
    let foreign_identity = process::identity(foreign.0.id()).unwrap();
    fsx::atomic_write_json(&root.join(".state/world.json"), &foreign_identity)?;
    let rejected = driver::force_stop(root)?;
    assert!(!rejected.ok, "a foreign world record must be rejected");
    assert!(!root.join("database-stage").exists());
    assert!(foreign.0.try_wait()?.is_none());
    let mut owned = spawn(&root.join("Core/worldserver.exe"));
    let stopped = driver::force_stop(root)?;
    assert!(stopped.ok, "{}", stopped.output);
    assert!(owned.0.try_wait()?.is_some());
    assert!(
        foreign.0.try_wait()?.is_none(),
        "foreign process was untouched"
    );
    assert_eq!(std::fs::read(root.join("database-stage"))?, b"safe");
    println!("PASS: native Windows forced-stop rejects foreign records, recovers the exact owned world identity, stops it before the database stage, and leaves the foreign process alive");
    Ok(())
}
