//! Real MSVC + CMake + Ninja: after the build cache is packed and unpacked, a rebuild with nothing changed
//! compiles nothing, and a rebuild after one source changed compiles only that source.
//! usage: build_cache_ninja_check <empty folder>
use std::path::Path;
use std::process::Command;

fn build(root: &Path, dir: &Path, tc: &coa_core::build_engine::ToolchainStatus) -> String {
    let bat = root.join("run.bat");
    let ninja = Path::new(tc.ninja_path.as_ref().unwrap()).parent().unwrap().display().to_string();
    std::fs::write(&bat, format!(
        "@echo off\r\ncall \"{}\" > nul 2>&1\r\nset \"PATH={};%PATH%\"\r\n\"{}\" -G Ninja -S \"{}\" -B \"{}\\build\" -DCMAKE_BUILD_TYPE=RelWithDebInfo > nul\r\n\"{}\" --build \"{}\\build\" -- -d explain\r\n",
        tc.vcvars64_path.as_ref().unwrap(), ninja, tc.cmake_path.as_ref().unwrap(), dir.display(), dir.display(), tc.cmake_path.as_ref().unwrap(), dir.display()
    )).unwrap();
    let out = Command::new("cmd.exe").args(["/c", &bat.to_string_lossy()]).output().unwrap();
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

fn compiled(log: &str) -> usize {
    log.lines().filter(|l| l.contains("Building CXX object")).count()
}

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: build_cache_ninja_check <empty folder>"));
    assert!(!root.exists() || std::fs::read_dir(&root).unwrap().next().is_none(), "folder must be new or empty");
    std::fs::create_dir_all(root.join(".manager")).unwrap();
    let tc = coa_core::build_engine::detect_toolchain();
    assert!(tc.msvc_available && tc.cmake_available && tc.ninja_available, "toolchain: {tc:?}");

    let dir = coa_core::build_cache::prepare(&root, "probe", "fp").unwrap();
    std::fs::write(dir.join("CMakeLists.txt"), "cmake_minimum_required(VERSION 3.22)\nproject(p CXX)\nadd_library(p SHARED a.cpp b.cpp c.cpp)\n").unwrap();
    for f in ["a", "b", "c"] {
        std::fs::write(dir.join(format!("{f}.cpp")), format!("#include <vector>\n#include <string>\n__declspec(dllexport) int {f}() {{ return (int)std::vector<std::string>(3).size(); }}\n")).unwrap();
    }
    let first = build(&root, &dir, &tc);
    println!("first build compiled {}", compiled(&first));
    assert_eq!(compiled(&first), 3);

    coa_core::build_cache::finish(&root, "probe");
    let s = coa_core::build_cache::summary(&root, None);
    println!("packed: {} bytes from {} unpacked", s.total_bytes, s.modules[0].unpacked_bytes);
    assert!(!dir.exists());

    let dir = coa_core::build_cache::prepare(&root, "probe", "fp").unwrap();
    let again = build(&root, &dir, &tc);
    println!("after unpack, nothing changed: compiled {}\n{}", compiled(&again), again.lines().filter(|l| l.contains("explain")).take(8).collect::<Vec<_>>().join("\n"));
    assert_eq!(compiled(&again), 0, "nothing should be recompiled");
    coa_core::build_cache::finish(&root, "probe");

    let dir = coa_core::build_cache::prepare(&root, "probe", "fp").unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    std::fs::write(dir.join("b.cpp"), "__declspec(dllexport) int b() { return 2; }\n").unwrap();
    let one = build(&root, &dir, &tc);
    println!("after one change: compiled {}", compiled(&one));
    assert_eq!(compiled(&one), 1);
    coa_core::build_cache::finish(&root, "probe");
    println!("PASS");
}
