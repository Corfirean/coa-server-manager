//! Controlled Build Engine for CoA Dynamic Plugin Architecture.
//!
//! Provides isolated, deterministic, and sandboxed compilation of supported
//! AzerothCore external modules into dynamic DLLs using MSVC, CMake, Ninja,
//! and the CoA Plugin SDK v0.3.2.
//!
//! Enforces:
//! - Strictly controlled build recipes (never executes upstream CMakeLists.txt or arbitrary scripts)
//! - Automatically generated C ABI adapter
//! - Single active concurrent compilation lock
//! - Real-time progress and MSVC diagnostics extraction
//! - Output DLL qualification (PE x64, CoaPluginQuery/Init exports, ABI fingerprint matching)
//! - Transactional packaging into CSM Plugin Store

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::analyzer::{generate_adapter, AdapterGenerationResult, VerificationStatus};
use crate::error::{Error, Result};
use crate::provenance::parse_pe_exports;

static BUILD_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainStatus {
    pub msvc_available: bool,
    pub vcvars64_path: Option<String>,
    /// MSVC toolset directory passed to vcvars64 as `-vcvars_ver` (for example 14.44.35207).
    #[serde(default)]
    pub msvc_toolset: Option<String>,
    /// Version of that toolset's cl.exe (for example 19.44.35229).
    #[serde(default)]
    pub msvc_cl_version: Option<String>,
    /// The exact cl.exe version the realm's SDK was built with; plugins built with another one get
    /// another ABI fingerprint and are refused by the core.
    #[serde(default)]
    pub required_cl_version: Option<String>,
    pub cmake_available: bool,
    pub cmake_path: Option<String>,
    pub ninja_available: bool,
    pub ninja_path: Option<String>,
    pub sdk_available: bool,
    pub sdk_dir: Option<String>,
    pub sdk_abi_fingerprint: Option<String>,
    pub ready: bool,
    pub diagnostic_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildErrorDiagnostic {
    pub file: String,
    pub line: u32,
    pub error_code: String,
    pub message: String,
    pub explanation: Option<String>,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProgressEvent {
    pub step: String,
    pub progress_pct: u32,
    pub message: String,
    pub log_line: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildResult {
    pub success: bool,
    pub module_id: String,
    pub module_name: String,
    pub display_name: String,
    pub version: String,
    pub source_commit: Option<String>,
    pub package_path: Option<String>,
    pub dll_path: Option<String>,
    pub dll_sha256: Option<String>,
    pub abi_fingerprint: Option<String>,
    pub duration_seconds: f64,
    pub errors: Vec<BuildErrorDiagnostic>,
    pub log_output: Vec<String>,
    pub verification_status: Option<VerificationStatus>,
}

/// Where a realm keeps the plugin SDKs of its cores: `<realm>/.manager/sdk/<first 16 hex of the host
/// fingerprint>/`. The full fingerprint is checked against the SDK's sdk.json; the short name keeps header
/// paths in Ninja's dependency log under Windows' 260-character limit (beyond it Ninja cannot stat a
/// header, treats it as changed and recompiles everything on every build).
pub fn realm_sdk_store(realm_root: &Path) -> PathBuf {
    realm_root.join(".manager").join("sdk")
}

/// The SDK directory of one host fingerprint inside the realm's store.
pub fn realm_sdk_dir(realm_root: &Path, host_fp: &str) -> PathBuf {
    realm_sdk_store(realm_root).join(&host_fp[..host_fp.len().min(16)])
}

/// The cmake directory of the SDK that matches the realm's installed worldserver.exe: a plugin built with
/// any other SDK would be rejected by this host. None when the host records no ABI or no SDK for it is
/// installed.
pub fn realm_sdk_cmake_dir(realm_root: &Path) -> Option<PathBuf> {
    let host_fp = crate::provenance::realm_host_fingerprint(realm_root)?;
    let cmake = realm_sdk_dir(realm_root, &host_fp).join("cmake");
    let ready = |cmake: &Path| {
        cmake.join("CoAPluginSDKConfig.cmake").is_file()
            && sdk_abi_fingerprint(cmake).ok().as_deref() == Some(host_fp.as_str())
    };
    if ready(&cmake) {
        return Some(cmake);
    }
    // The core package ships its SDK as Core/plugin-sdk.zip; unpack it once into the store.
    match unpack_bundled_sdk(realm_root, &host_fp) {
        Ok(true) if ready(&cmake) => Some(cmake),
        _ => None,
    }
}

/// The SDK archive a core package ships next to worldserver.exe (repack and flat layouts).
fn bundled_sdk_zip(realm_root: &Path) -> Option<PathBuf> {
    [realm_root.join("Core").join("plugin-sdk.zip"), realm_root.join("plugin-sdk.zip")]
        .into_iter()
        .find(|p| p.is_file())
}

/// Unpacks the bundled SDK into the realm's store when it belongs to the installed host. Returns false when
/// there is no bundled SDK or it is for another core (an update replaced worldserver.exe but not the zip).
fn unpack_bundled_sdk(realm_root: &Path, host_fp: &str) -> Result<bool> {
    use std::io::Read;
    let Some(zip_path) = bundled_sdk_zip(realm_root) else { return Ok(false) };
    let mut archive = zip::ZipArchive::new(fs::File::open(&zip_path)?)
        .map_err(|e| Error::Invalid(format!("[SDK_ARCHIVE] {}: {}", zip_path.display(), e)))?;
    let entry_name = |raw: &str| raw.replace('\\', "/");
    let mut sdk_json = String::new();
    {
        let idx = (0..archive.len())
            .find(|&i| archive.by_index(i).map(|e| entry_name(e.name()) == "sdk.json").unwrap_or(false));
        let Some(idx) = idx else { return Ok(false) };
        archive.by_index(idx).map_err(|e| Error::Invalid(e.to_string()))?.read_to_string(&mut sdk_json)?;
    }
    let zip_fp = serde_json::from_str::<serde_json::Value>(&sdk_json)
        .ok()
        .and_then(|v| v.get("abiFingerprint").and_then(|f| f.as_str()).map(str::to_owned));
    if zip_fp.as_deref() != Some(host_fp) {
        return Ok(false);
    }
    let store = realm_sdk_store(realm_root);
    let staging = store.join(format!(".incoming-{}", &host_fp[..host_fp.len().min(16)]));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| Error::Invalid(e.to_string()))?;
        let name = entry_name(entry.name());
        let rel = Path::new(&name);
        if rel.is_absolute() || rel.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(Error::Invalid(format!("[SDK_ARCHIVE] unsafe entry '{}' in {}", name, zip_path.display())));
        }
        let dest = staging.join(rel);
        if name.ends_with('/') {
            fs::create_dir_all(&dest)?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        std::io::copy(&mut entry, &mut fs::File::create(&dest)?)?;
    }
    let dest = realm_sdk_dir(realm_root, host_fp);
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::rename(&staging, &dest)?;
    Ok(true)
}

/// Installs an SDK (a directory with sdk.json, cmake/, include/, lib/) for the realm's current core. The SDK
/// must have been exported from that core: its fingerprint must equal the one worldserver.exe records.
pub fn install_realm_sdk(realm_root: &Path, sdk_root: &Path) -> Result<PathBuf> {
    let host_fp = crate::provenance::realm_host_fingerprint(realm_root).ok_or_else(|| {
        Error::Invalid(format!(
            "[SDK_HOST_UNKNOWN] The worldserver.exe of '{}' records no plugin ABI; it is not a plugin host.",
            realm_root.display()
        ))
    })?;
    let sdk_fp = sdk_abi_fingerprint(&sdk_root.join("cmake"))?;
    if sdk_fp != host_fp {
        return Err(Error::Invalid(format!(
            "[SDK_HOST_MISMATCH] SDK '{}' is for ABI {}, the realm's core is {}.",
            sdk_root.display(), sdk_fp, host_fp
        )));
    }
    let store = realm_sdk_store(realm_root);
    let dest = realm_sdk_dir(realm_root, &host_fp);
    if realm_sdk_cmake_dir(realm_root).is_some() {
        return Ok(dest);
    }
    fs::create_dir_all(&store)?;
    let staging = store.join(format!(".incoming-{}", &host_fp[..host_fp.len().min(16)]));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    copy_dir_all(sdk_root, &staging)?;
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::rename(&staging, &dest)?;
    Ok(dest)
}

fn copy_dir_all(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Detects the MSVC toolchain, CMake and Ninja, without a realm (so without an SDK unless
/// `COA_PLUGIN_SDK_DIR` names one).
pub fn detect_toolchain() -> ToolchainStatus {
    detect_toolchain_for(None)
}

/// Detects the MSVC toolchain, CMake, Ninja, and the CoA Plugin SDK for `realm_root`'s core.
/// `COA_PLUGIN_SDK_DIR` overrides the SDK (development only; the build still checks it against the host).
pub fn detect_toolchain_for(realm_root: Option<&Path>) -> ToolchainStatus {
    let installs = visual_studio_installs();
    let ide_tool = |rel: &str| installs.iter().map(|i| i.join(rel)).find(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned());

    let cmake_path = std::env::var("COA_CMAKE").ok().filter(|p| Path::new(p).is_file())
        .or_else(|| ide_tool("Common7\\IDE\\CommonExtensions\\Microsoft\\CMake\\CMake\\bin\\cmake.exe"))
        .or_else(|| which_cmd("cmake.exe"));
    let ninja_path = std::env::var("COA_NINJA").ok().filter(|p| Path::new(p).is_file())
        .or_else(|| ide_tool("Common7\\IDE\\CommonExtensions\\Microsoft\\CMake\\Ninja\\ninja.exe"))
        .or_else(|| which_cmd("ninja.exe"));

    let sdk_dir = std::env::var("COA_PLUGIN_SDK_DIR").ok().filter(|p| Path::new(p).join("CoAPluginSDKConfig.cmake").is_file())
        .or_else(|| realm_root.and_then(realm_sdk_cmake_dir).map(|p| p.to_string_lossy().into_owned()));

    // The compiler must be the one the SDK names, to the build number: it is part of the fingerprint.
    let required_cl_version = sdk_dir.as_deref().and_then(|d| sdk_cl_version(Path::new(d)));
    let toolsets = msvc_toolsets(&installs);
    let chosen = match &required_cl_version {
        Some(want) => toolsets.iter().find(|t| &t.cl_version == want),
        None => toolsets.iter().max_by(|a, b| version_key(&a.cl_version).cmp(&version_key(&b.cl_version))),
    };
    let (vcvars64_path, msvc_toolset, msvc_cl_version) = match std::env::var("COA_VCVARS64").ok().filter(|p| Path::new(p).is_file()) {
        Some(over) => (Some(over), None, None),
        None => match chosen {
            Some(t) => (Some(t.vcvars64.to_string_lossy().into_owned()), Some(t.toolset.clone()), Some(t.cl_version.clone())),
            None => (None, None, None),
        },
    };

    let msvc_available = vcvars64_path.is_some();
    let cmake_available = cmake_path.is_some();
    let ninja_available = ninja_path.is_some();
    let sdk_available = sdk_dir.is_some();
    let ready = msvc_available && cmake_available && ninja_available && sdk_available;

    let diagnostic_message = if ready {
        "MSVC, CMake, Ninja, and the CoA Plugin SDK for this core are ready.".into()
    } else {
        let mut missing = Vec::new();
        let msvc_missing;
        if !msvc_available {
            msvc_missing = match &required_cl_version {
                Some(want) => format!("MSVC {} (Visual Studio Build Tools with the toolset this core's SDK names; installed: {})",
                    want, if toolsets.is_empty() { "none".to_string() } else {
                        toolsets.iter().map(|t| t.cl_version.as_str()).collect::<Vec<_>>().join(", ") }),
                None => "MSVC (Visual Studio Build Tools)".to_string(),
            };
            missing.push(msvc_missing.as_str());
        }
        if !cmake_available { missing.push("CMake"); }
        if !ninja_available { missing.push("Ninja"); }
        if !sdk_available { missing.push("CoA Plugin SDK for this realm's core"); }
        format!("Build toolchain incomplete. Missing: {}", missing.join(", "))
    };

    ToolchainStatus {
        msvc_available,
        vcvars64_path,
        msvc_toolset,
        msvc_cl_version,
        required_cl_version,
        cmake_available,
        cmake_path,
        ninja_available,
        ninja_path,
        sdk_available,
        sdk_abi_fingerprint: sdk_dir.as_deref().and_then(|d| sdk_abi_fingerprint(Path::new(d)).ok()),
        sdk_dir,
        ready,
        diagnostic_message,
    }
}

/// One installed MSVC toolset: the vcvars64.bat of its Visual Studio and the toolset directory name.
#[derive(Debug, Clone)]
struct MsvcToolset {
    vcvars64: PathBuf,
    toolset: String,
    cl_version: String,
}

/// Installation folders of every Visual Studio / Build Tools product, found with vswhere (installed
/// with any of them).
fn visual_studio_installs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:\\Program Files (x86)".into());
    let vswhere = Path::new(&pf86).join("Microsoft Visual Studio").join("Installer").join("vswhere.exe");
    if vswhere.is_file() {
        let mut cmd = Command::new(&vswhere);
        cmd.args(["-all", "-prerelease", "-products", "*", "-sort", "-property", "installationPath", "-utf8"]);
        hide_window(&mut cmd);
        if let Ok(o) = cmd.output() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let p = PathBuf::from(line.trim());
                if !line.trim().is_empty() && p.is_dir() && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Every MSVC x64 toolset under `installs`, with the version its cl.exe reports.
fn msvc_toolsets(installs: &[PathBuf]) -> Vec<MsvcToolset> {
    let mut out = Vec::new();
    for inst in installs {
        // A minimal Build Tools install (just the toolset component) has no vcvars64.bat; VsDevCmd.bat
        // is always there and takes the same -vcvars_ver.
        let Some(vcvars64) = [
            inst.join("VC").join("Auxiliary").join("Build").join("vcvars64.bat"),
            inst.join("Common7").join("Tools").join("VsDevCmd.bat"),
        ].into_iter().find(|p| p.is_file()) else { continue };
        let Ok(dirs) = fs::read_dir(inst.join("VC").join("Tools").join("MSVC")) else { continue };
        for d in dirs.flatten() {
            let cl = d.path().join("bin").join("Hostx64").join("x64").join("cl.exe");
            if let Some(cl_version) = cl.is_file().then(|| cl_exe_version(&cl)).flatten() {
                out.push(MsvcToolset { vcvars64: vcvars64.clone(), toolset: d.file_name().to_string_lossy().into_owned(), cl_version });
            }
        }
    }
    out
}

/// The version cl.exe prints in its banner (`19.44.35229`); the banner text itself is localised.
fn cl_exe_version(cl: &Path) -> Option<String> {
    let mut cmd = Command::new(cl);
    hide_window(&mut cmd);
    let o = cmd.output().ok()?;
    parse_cl_version(&String::from_utf8_lossy(&o.stderr))
}

fn parse_cl_version(banner: &str) -> Option<String> {
    banner.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .find(|w| w.starts_with("19.") && w.split('.').count() >= 3)
        .map(|w| w.split('.').take(3).collect::<Vec<_>>().join("."))
}

/// The cl.exe version an SDK was built with, from its `compiler=MSVC-19.44.35229.0` ABI input.
fn sdk_cl_version(sdk_cmake_dir: &Path) -> Option<String> {
    let path = [sdk_cmake_dir.join("sdk.json"), sdk_cmake_dir.join("..").join("sdk.json")].into_iter().find(|p| p.is_file())?;
    let json: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let compiler = json.get("abiInputs")?.as_array()?.iter()
        .filter_map(|v| v.as_str())
        .find_map(|i| i.strip_prefix("compiler=MSVC-"))?;
    Some(compiler.split('.').take(3).collect::<Vec<_>>().join("."))
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn which_cmd(cmd_name: &str) -> Option<String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(cmd_name);
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// The ABI fingerprint of an SDK, from the sdk.json written next to its cmake directory.
fn sdk_abi_fingerprint(sdk_cmake_dir: &Path) -> Result<String> {
    let candidates = [sdk_cmake_dir.join("sdk.json"), sdk_cmake_dir.join("..").join("sdk.json")];
    let path = candidates.iter().find(|p| p.is_file()).ok_or_else(|| {
        Error::Invalid(format!("CoA Plugin SDK at {} has no sdk.json", sdk_cmake_dir.display()))
    })?;
    let json: serde_json::Value = serde_json::from_slice(&fs::read(path)?)
        .map_err(|e| Error::Invalid(format!("{}: {}", path.display(), e)))?;
    json.get("abiFingerprint")
        .and_then(|v| v.as_str())
        .filter(|fp| fp.len() == 64 && fp.bytes().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(|| Error::Invalid(format!("{} has no valid abiFingerprint", path.display())))
}

/// Copies `src` to `dest` only when the content differs, so an unchanged file keeps its timestamp and
/// Ninja does not rebuild it.
fn sync_file(src: &Path, dest: &Path) -> Result<()> {
    let data = fs::read(src)?;
    write_if_changed(dest, &data)
}

fn write_if_changed(path: &Path, data: &[u8]) -> Result<()> {
    if fs::read(path).map(|old| old == data).unwrap_or(false) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, data)?;
    Ok(())
}

/// Deletes staged sources that no longer exist upstream (renamed or removed files).
fn remove_unstaged(dir: &Path, keep: &std::collections::HashSet<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_unstaged(&path, keep)?;
        } else if !keep.contains(&path) {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

/// The plugin id an analyzer-generated adapter declares.
/// `adapter_source` with the id in its COA_PLUGIN_DEFINE replaced by `plugin_id`.
fn with_plugin_id(adapter_source: &str, plugin_id: &str) -> String {
    match adapter_plugin_id(adapter_source) {
        Some(old) => adapter_source.replacen(&format!("COA_PLUGIN_DEFINE(\"{old}\""), &format!("COA_PLUGIN_DEFINE(\"{plugin_id}\""), 1),
        None => adapter_source.to_string(),
    }
}

fn adapter_plugin_id(adapter_source: &str) -> Option<String> {
    let marker = "COA_PLUGIN_DEFINE(\"";
    let start = adapter_source.find(marker)? + marker.len();
    let end = adapter_source[start..].find('"')?;
    Some(adapter_source[start..start + end].to_string())
}

/// Adapter for modules whose shutdown work runs in hooks, and for bundles. A faulted plugin's hooks are
/// suppressed but its lifecycle phases still run, so the bot modules save and log out their bots in
/// onStopping as well (running it twice is harmless: the second pass finds no bot in the world).
/// SQUID's world-thread queue is cleared in onDrainBeforeDbClose: its static destructor otherwise logs
/// through the core's already destroyed Log at process exit (0xC0000005).
fn lifecycle_adapter(module_id: &str, plugin_id: &str, loader: &str, bundled_loaders: &[String]) -> Option<String> {
    let (includes, stopping, drain): (&[&str], &str, &str) = match module_id {
        "companions" => (&["BotMgr.h"], "if (BotMgr::BotsEnabled())\n            sBotMgr->LogoutAllBots();", ""),
        "playerbots" => (
            &["RandomPlayerbotMgr.h", "PlayerbotWorldThreadProcessor.h", "PlayerbotCommandServer.h"],
            "sRandomPlayerbotMgr.LogoutAllBots();",
            "PlayerbotWorldThreadProcessor::instance().ClearQueue();\n        PlayerbotCommandServer::instance().Stop();",
        ),
        _ if !bundled_loaders.is_empty() => (&[], "", ""),
        _ => return None,
    };
    let loaders: Vec<&str> = std::iter::once(loader).chain(bundled_loaders.iter().map(String::as_str)).collect();
    let mut out = String::from("#include \"CoaPluginApi.h\"\n");
    for h in includes {
        out += &format!("#include \"{}\"\n", h);
    }
    out += "\n";
    for l in &loaders {
        out += &format!("void {}();\n", l);
    }
    out += "\nnamespace\n{\n    bool Register()\n    {\n";
    for l in &loaders {
        out += &format!("        {}();\n", l);
    }
    out += "        return true;\n    }\n\n    bool Startup()\n    {\n        return true;\n    }\n\n";
    out += &format!("    void Stopping()\n    {{\n        {}\n    }}\n\n", stopping);
    out += &format!("    bool Drain()\n    {{\n        {}\n        return true;\n    }}\n\n", drain);
    out += "    void Finalize()\n    {\n    }\n}\n\n";
    out += &format!("COA_PLUGIN_DEFINE(\"{}\", \"ext\", Register, Startup, Stopping, Drain, Finalize)\n", plugin_id);
    Some(out)
}

/// Computes SHA-256 of file bytes.
fn sha256_of_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Directories of a module repository that are never compiled into its plugin.
pub(crate) fn is_unbuilt_dir(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(),
        "tests" | "test" | "stubs" | "stub" | "pilot" | "tools" | "build" | ".git" | ".github" | "cmake")
}

/// Recursively collects all files with specified extension, skipping non-module directories.
fn collect_files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut results = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if is_unbuilt_dir(name) {
                    continue;
                }
                results.extend(collect_files_with_ext(&path, ext));
            } else if let Some(e) = path.extension() {
                if e.to_string_lossy().eq_ignore_ascii_case(ext) {
                    results.push(path);
                }
            }
        }
    }
    results
}

/// Sanitizes module identifier for CMake targets and filenames.
pub fn sanitize_module_name(name: &str) -> String {
    name.replace('-', "_")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// Decodes compiler / linker stdout/stderr bytes, ensuring valid UTF-8 without mojibake.
pub fn decode_msvc_output(bytes: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b < 0x80 {
            out.push(b as char);
        } else if (0x80..=0xAF).contains(&b) {
            // CP866: А..п
            if let Some(c) = char::from_u32(0x0410 + (b - 0x80) as u32) {
                out.push(c);
            } else {
                out.push('?');
            }
        } else if (0xE0..=0xEF).contains(&b) {
            // CP866: р..я
            if let Some(c) = char::from_u32(0x0440 + (b - 0xE0) as u32) {
                out.push(c);
            } else {
                out.push('?');
            }
        } else if b == 0xF0 {
            out.push('Ё');
        } else if b == 0xF1 {
            out.push('ё');
        } else if (0xC0..=0xFF).contains(&b) {
            // CP1251: А..я
            if let Some(c) = char::from_u32(0x0410 + (b - 0xC0) as u32) {
                out.push(c);
            } else {
                out.push('?');
            }
        } else {
            out.push('?');
        }
    }
    out
}

/// Resolves a canonical module identity, readable display name, and optional commit SHA.
pub fn resolve_module_identity(source_dir: &Path) -> (String, String, Option<String>) {
    let raw_name = source_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "custom_module".into());

    let (canonical_raw, commit_sha) = if raw_name.len() == 40 && raw_name.chars().all(|c| c.is_ascii_hexdigit()) {
        let repo_dir = source_dir.parent();
        let repo_name = repo_dir
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw_name.clone());
        (repo_name, Some(raw_name))
    } else {
        (raw_name, None)
    };

    let raw_id = canonical_raw.trim_start_matches("mod-").replace('_', "-");

    let (module_id, display_name) = match raw_id.as_str() {
        "coa-content-scaling" | "content-scaling" | "scaling" => {
            ("content-scaling".to_string(), "Content Scaling".to_string())
        }
        "coa-playerbots" | "companions" => {
            ("companions".to_string(), "CoA Companions".to_string())
        }
        "squid-playerbots" | "playerbots" | "squid" => {
            ("playerbots".to_string(), "SQUID's Playerbots".to_string())
        }
        "dungeon-clear" | "mod-dungeon-clear" => {
            ("dungeon-clear".to_string(), "Dungeon Clear".to_string())
        }
        "npc-buffer" => ("npc-buffer".to_string(), "NPC Buffer".to_string()),
        "ah-bot" => ("ah-bot".to_string(), "Auction House Bot".to_string()),
        "guildhouse" => ("guildhouse".to_string(), "Guildhouse".to_string()),
        "congrats-on-level" => ("congrats-on-level".to_string(), "Congrats on Level".to_string()),
        "npc-enchanter" => ("npc-enchanter".to_string(), "NPC Enchanter".to_string()),
        "reward-played-time" => ("reward-played-time".to_string(), "Reward Played Time".to_string()),
        "transmog" => ("transmog".to_string(), "Transmog".to_string()),
        "solocraft" => ("solocraft".to_string(), "Solocraft".to_string()),
        "autobalance" => ("autobalance".to_string(), "AutoBalance".to_string()),
        "individual-progression" => ("individual-progression".to_string(), "Individual Progression".to_string()),
        "ale" => ("ale".to_string(), "AzerothCore Lua Engine (ALE)".to_string()),
        "custom-races" => ("custom-races".to_string(), "Custom Races".to_string()),
        "timeistime" | "time-is-time" => ("time-is-time".to_string(), "TimeIsTime".to_string()),
        other => {
            let pretty = other.split('-')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            (other.to_string(), pretty)
        }
    };

    (module_id, display_name, commit_sha)
}

/// Parses MSVC compiler and linker error messages from stdout/stderr.
/// Ninja records headers relative to the build directory and stats `<build dir>\<relative path>` with the
/// classic Win32 API, which fails past 260 characters unless long paths are enabled. A header it cannot
/// stat counts as changed, so every build silently becomes a full rebuild. Refuse such a layout up front.
const NINJA_PATH_LIMIT: usize = 250;

fn check_dependency_path_lengths(build_dir: &Path, sdk_cmake_dir: &Path) -> Result<()> {
    let sdk_root = sdk_cmake_dir.parent().unwrap_or(sdk_cmake_dir);
    let include = sdk_root.join("include");
    let longest = longest_relative_file(&include);
    let Some(longest) = longest else { return Ok(()) };
    let header = include.join(&longest);
    let worst = ninja_stat_path_len(build_dir, &header);
    if worst > NINJA_PATH_LIMIT {
        return Err(Error::Invalid(format!(
            "[BUILD_PATH_TOO_LONG] Header paths would reach {} characters ({}), over the Windows limit Ninja can handle ({}). Move the server to a shorter folder (for example C:/CoA) or enable Windows long paths.",
            worst, header.display(), NINJA_PATH_LIMIT
        )));
    }
    Ok(())
}

/// The longest file path below `root`, relative to it.
fn longest_relative_file(root: &Path) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                if best.as_ref().map_or(true, |b| rel.as_os_str().len() > b.as_os_str().len()) {
                    best = Some(rel.to_path_buf());
                }
            }
        }
    }
    best
}

/// Length of `<build dir>\<path of target relative to build dir>`, as Ninja forms it.
fn ninja_stat_path_len(build_dir: &Path, target: &Path) -> usize {
    let norm = |p: &Path| -> Vec<String> {
        p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()
    };
    let (b, t) = (norm(build_dir), norm(target));
    let common = b.iter().zip(t.iter()).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return target.as_os_str().len(); // another drive: Ninja keeps the absolute path
    }
    let ups = b.len() - common;
    let rest: usize = target.components().skip(common).map(|c| c.as_os_str().len() + 1).sum();
    build_dir.as_os_str().len() + 1 + ups * 3 + rest.saturating_sub(1)
}

/// Parallel compile jobs: one per core, but no more than free RAM allows (a SQUID translation unit with
/// /bigobj takes up to ~2.5 GB in cl.exe; running out of memory kills the build without an MSVC error).
fn build_jobs() -> u32 {
    let hw = crate::population::hardware();
    let by_ram = (hw.free_ram_gb / 2.5).floor() as u32;
    hw.cores.min(by_ram).clamp(1, 16)
}

/// A failure without a recognised MSVC diagnostic (out of memory, a crashed tool, a CMake error): the
/// step that failed and the end of the log, so the user never sees an empty error list.
fn unparsed_failure(exit_code: i32, logs: &[String], log_path: &Path) -> BuildErrorDiagnostic {
    let step = match exit_code {
        101 => "Visual Studio environment (vcvars64) could not be loaded",
        102 => "CMake configuration failed",
        103 => "Compilation or linking failed",
        _ => "Build runner failed",
    };
    let tail: Vec<&str> = logs.iter().map(String::as_str)
        .filter(|l| !l.contains("Note: including file") && !l.trim().is_empty())
        .rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
    BuildErrorDiagnostic {
        file: log_path.to_string_lossy().into_owned(),
        line: 0,
        error_code: format!("BUILD_EXIT_{}", exit_code),
        message: format!("{} (exit code {}). Last output:
{}", step, exit_code, tail.join("
")),
        explanation: None,
        resolution: Some(format!("Full log: {}", log_path.display())),
    }
}

fn parse_msvc_errors(logs: &[String]) -> Vec<BuildErrorDiagnostic> {
    let mut diagnostics = Vec::new();
    for line in logs {
        // MSVC C-error: filename(line,col): [fatal] error Cxxxx: message
        if line.contains(": error C") || line.contains(": fatal error C") {
            let (left, right) = if let Some(idx) = line.find(": fatal error ") {
                (line[..idx].trim(), line[idx + 14..].trim())
            } else if let Some(idx) = line.find(": error ") {
                (line[..idx].trim(), line[idx + 8..].trim())
            } else {
                ("", line.trim())
            };

            if !right.is_empty() {
                let (file, line_num) = if let Some(idx) = left.rfind('(') {
                    let f = left[..idx].trim().to_string();
                    let rest = &left[idx + 1..];
                    let l = rest.split([',', ')']).next().unwrap_or("0").parse::<u32>().unwrap_or(0);
                    (f, l)
                } else {
                    (left.to_string(), 0)
                };

                let error_code = right.split(':').next().unwrap_or("").trim().to_string();
                let msg = right.split(':').skip(1).collect::<Vec<&str>>().join(":").trim().to_string();
                let clean_msg = if msg.is_empty() { right.to_string() } else { msg };

                diagnostics.push(BuildErrorDiagnostic {
                    file,
                    line: line_num,
                    error_code,
                    message: clean_msg,
                    explanation: None,
                    resolution: None,
                });
            }
        } else if line.contains("error LNK") {
            let code = if line.contains("LNK2019") {
                "LNK2019"
            } else if line.contains("LNK2001") {
                "LNK2001"
            } else if line.contains("LNK1120") {
                "LNK1120"
            } else {
                "LINKER_ERROR"
            };

            let (file, msg) = if let Some(idx) = line.find(" : error ") {
                (line[..idx].trim().to_string(), line[idx + 9..].trim().to_string())
            } else {
                ("linker".to_string(), line.trim().to_string())
            };

            let (explanation, resolution) = if msg.contains("Player::~Player") || msg.contains("??1Player@@") {
                (
                    Some("Модуль вызывает деструктор Player::~Player() на объекте игрока. В SDK v0.3.2 этот экспорт добавлен.".to_string()),
                    Some("Обновите SDK до v0.3.2 или используйте Host PlayerServices.".to_string()),
                )
            } else if msg.contains("OnAuctionSuccessful") || msg.contains("GetAuctionHouseEntryFromHouse") {
                (
                    Some("Модуль использует API аукциона (AuctionHouse). В SDK v0.3.2 эти экспорты добавлены.".to_string()),
                    Some("Обновите SDK до v0.3.2.".to_string()),
                )
            } else if msg.contains("GameObject") || msg.contains("Creature") || msg.contains("SaveToDB") || msg.contains("DeleteFromDB") || msg.contains("AddObjectToRemoveList") || msg.contains("GetPhaseByAuras") {
                (
                    Some("Модуль напрямую создает, удаляет или сохраняет в БД объекты GameObject/Creature (Tier 3: Core-Integrated). Прямое управление памятью ядра не поддерживается динамическими DLL.".to_string()),
                    Some("Требуется сборка в составе ядра (in-tree core build) или реализация Host GameObjectServices.".to_string()),
                )
            } else {
                (None, None)
            };

            diagnostics.push(BuildErrorDiagnostic {
                file,
                line: 0,
                error_code: code.to_string(),
                message: msg,
                explanation,
                resolution,
            });
        }
    }
    diagnostics
}

/// Performs a controlled, deterministic compilation of an AzerothCore module.
pub fn build_module_from_source(
    module_source_dir: &Path,
    realm_root: &Path,
    custom_staging_dir: Option<&Path>,
    progress_callback: Option<Box<dyn Fn(BuildProgressEvent) + Send>>,
) -> Result<BuildResult> {
    build_module_with_bundle(module_source_dir, &[], realm_root, custom_staging_dir, progress_callback)
}

/// The sources a plugin was last built from, kept per realm so the manager can rebuild it on its own
/// after a core update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRecipe {
    pub schema: u32,
    pub source_dir: PathBuf,
    #[serde(default)]
    pub bundled: Vec<PathBuf>,
    #[serde(default)]
    pub source_commit: Option<String>,
}

fn build_recipe_path(realm_root: &Path, module_id: &str) -> PathBuf {
    crate::plugins::meta_dir(realm_root).join("build-recipes").join(format!("{module_id}.json"))
}

fn write_build_recipe(realm_root: &Path, module_id: &str, recipe: &BuildRecipe) -> Result<()> {
    let path = build_recipe_path(realm_root, module_id);
    fs::create_dir_all(path.parent().unwrap())?;
    crate::fsx::atomic_write(&path, serde_json::to_string_pretty(recipe)?.as_bytes())?;
    Ok(())
}

pub fn read_build_recipe(realm_root: &Path, module_id: &str) -> Option<BuildRecipe> {
    serde_json::from_slice(&fs::read(build_recipe_path(realm_root, module_id)).ok()?).ok()
}

/// Builds `module_source_dir` into one plugin DLL, together with `bundled` source trees that must live
/// in the same image (a module that derives from another module's C++ classes, such as Dungeon Clear
/// on SQUID). Each bundled tree's loader is called from the plugin's registration.
///
/// The staging and build directories persist between builds (per realm and module): sources are
/// synchronised file by file, unchanged files keep their timestamps, and Ninja recompiles only what
/// changed. Updating a bundled module therefore recompiles only its files and relinks the DLL.
pub fn build_module_with_bundle(
    module_source_dir: &Path,
    bundled: &[PathBuf],
    realm_root: &Path,
    custom_staging_dir: Option<&Path>,
    progress_callback: Option<Box<dyn Fn(BuildProgressEvent) + Send>>,
) -> Result<BuildResult> {
    let _lock = BUILD_LOCK.try_lock().map_err(|_| {
        Error::Invalid("Another module compilation is already in progress. Concurrent builds are restricted.".into())
    })?;

    let start_time = Instant::now();
    let mut all_logs = Vec::new();

    let emit_progress = |step: &str, pct: u32, msg: &str, log: Option<&str>| {
        if let Some(cb) = &progress_callback {
            cb(BuildProgressEvent {
                step: step.to_string(),
                progress_pct: pct,
                message: msg.to_string(),
                log_line: log.map(|s| s.to_string()),
            });
        }
    };

    emit_progress("PREPARING", 5, "Checking toolchain and build environment...", None);
    let toolchain = detect_toolchain_for(Some(realm_root));
    if !toolchain.ready {
        return Err(Error::Invalid(format!("[TOOLCHAIN_ERROR] {}", toolchain.diagnostic_message)));
    }
    // A plugin loads only on a host with the fingerprint it was built with: refuse to build one this
    // realm would reject (possible only with a COA_PLUGIN_SDK_DIR override).
    if let Some(host_fp) = crate::provenance::realm_host_fingerprint(realm_root) {
        if toolchain.sdk_abi_fingerprint.as_deref() != Some(host_fp.as_str()) {
            return Err(Error::Invalid(format!(
                "[SDK_HOST_MISMATCH] The SDK is for ABI {}, this realm's core is {}.",
                toolchain.sdk_abi_fingerprint.as_deref().unwrap_or("unknown"), host_fp
            )));
        }
    }

    let vcvars64 = toolchain.vcvars64_path.unwrap();
    let dev_cmd = Path::new(&vcvars64).file_name().is_some_and(|n| n.eq_ignore_ascii_case("VsDevCmd.bat"));
    let mut vcvars_ver = if dev_cmd { " -arch=amd64 -host_arch=amd64 -no_logo".to_string() } else { String::new() };
    if let Some(t) = toolchain.msvc_toolset.as_deref() {
        vcvars_ver.push_str(&format!(" -vcvars_ver={}", t));
    }
    let cmake = toolchain.cmake_path.unwrap();
    let ninja = toolchain.ninja_path.unwrap();
    let sdk_dir = toolchain.sdk_dir.unwrap();

    let (module_id, display_name, source_commit) = resolve_module_identity(module_source_dir);
    if let Some(reason) = crate::catalogue::is_blocked_repo(&module_id) {
        return Err(Error::Invalid(format!("[MODULE_BLOCKED] Module '{}' is blocked by CoA project policy: {}", module_id, reason)));
    }
    let raw_module_name = module_source_dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| module_id.clone());
    let safe_target_name = format!("mod_{}", sanitize_module_name(&module_id));

    emit_progress("ANALYZING", 15, "Analyzing module architecture and entrypoint...", None);

    // 1. Generate Adapter
    let adapter_res = generate_adapter(module_source_dir)?;
    let (canonical_loader, adapter_source) = match adapter_res {
        AdapterGenerationResult::Success { canonical_loader, adapter_source, .. } => {
            (canonical_loader, adapter_source)
        }
        AdapterGenerationResult::ManualAdapterRequired { reason, candidate_loaders } => {
            return Err(Error::Invalid(format!(
                "[MANUAL_ADAPTER_REQUIRED] {}: candidates [{}]",
                reason, candidate_loaders.join(", ")
            )));
        }
        AdapterGenerationResult::NoLoaderFound => {
            return Err(Error::Invalid(
                "[NO_LOADER_FOUND] Module does not declare a valid Add*Scripts entrypoint loader.".into()
            ));
        }
    };

    emit_progress("CONFIGURING", 30, "Generating controlled CMake recipe and staging sources...", None);

    // Every other loader the plugin must call: one per bundled source tree.
    let mut bundled_loaders = Vec::new();
    for tree in bundled {
        match generate_adapter(tree)? {
            AdapterGenerationResult::Success { canonical_loader, .. } => bundled_loaders.push(canonical_loader),
            _ => {
                return Err(Error::Invalid(format!(
                    "[NO_LOADER_FOUND] Bundled sources at '{}' have no single Add*Scripts loader.",
                    tree.display()
                )))
            }
        }
    }
    // The id inside the DLL is the one the registry and the plugin lock use: the core refuses a plugin
    // whose .coapl id differs from its lock entry.
    let plugin_id = module_id.clone();
    let adapter_source = lifecycle_adapter(&module_id, &plugin_id, &canonical_loader, &bundled_loaders)
        .unwrap_or_else(|| with_plugin_id(&adapter_source, &plugin_id));

    // 2. Prepare the persistent staging directory
    let default_staging = realm_root.join(".manager").join("build-cache").join(&module_id);
    let staging_dir = custom_staging_dir.map(PathBuf::from).unwrap_or(default_staging);
    fs::create_dir_all(&staging_dir)?;
    let staging_src = staging_dir.join("src");
    fs::create_dir_all(&staging_src)?;
    let build_output_dir = staging_dir.join("build");
    fs::create_dir_all(&build_output_dir)?;

    // Helper to strip verbatim extended path prefix (\\?\) so CMake and MSVC can parse include dirs
    let clean_path = |p: &Path| -> String {
        let s = p.to_string_lossy();
        let stripped = s.strip_prefix(r"\\?\").unwrap_or(&s);
        stripped.replace('\\', "/")
    };

    let adapter_path = staging_src.join("adapter.cpp");
    write_if_changed(&adapter_path, adapter_source.as_bytes())?;
    let mut staged: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    staged.insert(adapter_path.clone());

    let mut staged_cpp_paths = vec![clean_path(&adapter_path)];
    let mut include_dirs = vec![clean_path(&staging_src)];

    // Primary tree at the staging root, each bundled tree under bundle/<n>/. Every source is built as
    // it is: a symbol a module needs from the core must be exported by worldserver.exe, never
    // re-implemented (or stubbed) inside the plugin.
    let mut trees: Vec<(PathBuf, PathBuf)> = vec![(module_source_dir.to_path_buf(), staging_src.clone())];
    for (n, root) in bundled.iter().enumerate() {
        // A bundled repository is built from its src/ (its tests and tools stay out).
        let tree = if root.join("src").is_dir() { root.join("src") } else { root.clone() };
        trees.push((tree, staging_src.join("bundle").join(n.to_string())));
    }
    // A bundled tree inside the primary one (Content Scaling keeps its packs in content-packs/) is
    // compiled once, as the bundle: compiling it with the primary tree too defines its symbols twice.
    let bundled_roots: Vec<PathBuf> = bundled.iter().map(|b| b.canonicalize().unwrap_or_else(|_| b.clone())).collect();
    let in_bundle = |f: &Path| {
        let f = f.canonicalize().unwrap_or_else(|_| f.to_path_buf());
        bundled_roots.iter().any(|b| f.starts_with(b))
    };
    let mut source_count = 0;
    for (n, (tree, dest_root)) in trees.iter().enumerate() {
        let cpps: Vec<PathBuf> = collect_files_with_ext(tree, "cpp").into_iter()
            .filter(|f| n > 0 || !in_bundle(f)).collect();
        if cpps.is_empty() {
            return Err(Error::Invalid(format!("No C++ (.cpp) source files found in '{}'", tree.display())));
        }
        source_count += cpps.len();
        let headers: Vec<PathBuf> = collect_files_with_ext(tree, "h").into_iter()
            .chain(collect_files_with_ext(tree, "hpp")).collect();
        for file in cpps.iter().chain(headers.iter()) {
            let rel = file.strip_prefix(tree).unwrap_or_else(|_| Path::new(file.file_name().unwrap()));
            let dest = dest_root.join(rel);
            sync_file(file, &dest)?;
            staged.insert(dest.clone());
            if file.extension().map(|e| e.eq_ignore_ascii_case("cpp")).unwrap_or(false) {
                staged_cpp_paths.push(clean_path(&dest));
            }
        }
        include_dirs.push(clean_path(tree));
        for h in &headers {
            if let Some(parent) = h.parent() {
                let mut dirs = vec![parent.to_path_buf()];
                if let Ok(rel) = parent.strip_prefix(tree) {
                    dirs.push(dest_root.join(rel));
                }
                for dir in dirs {
                    let d = clean_path(&dir);
                    if !include_dirs.contains(&d) {
                        include_dirs.push(d);
                    }
                }
            }
        }
    }
    remove_unstaged(&staging_src, &staged)?;
    check_dependency_path_lengths(&build_output_dir, Path::new(&sdk_dir))?;
    emit_progress("CONFIGURING", 35, &format!(
        "Staged {} source files ({} tree(s)); unchanged files are not recompiled.", source_count, trees.len()), None);

    // 3. Write controlled CMakeLists.txt (eliminating D9025 /Zi vs /Z7 conflict)
    let sources_block = staged_cpp_paths.iter().map(|s| format!("  \"{}\"", s)).collect::<Vec<_>>().join("\n");
    let includes_block = include_dirs.iter().map(|s| format!("  \"{}\"", s)).collect::<Vec<_>>().join("\n");
    let cmake_sdk_path = clean_path(Path::new(&sdk_dir));

    let controlled_cmake = format!(r#"cmake_minimum_required(VERSION 3.22)
project(coa_plugin_{safe_target_name} LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_NINJA_FORCE_RESPONSE_FILE ON)

string(REPLACE "/Zi" "" CMAKE_CXX_FLAGS_RELWITHDEBINFO "${{CMAKE_CXX_FLAGS_RELWITHDEBINFO}}")
string(REPLACE "-Zi" "" CMAKE_CXX_FLAGS_RELWITHDEBINFO "${{CMAKE_CXX_FLAGS_RELWITHDEBINFO}}")

find_package(CoAPluginSDK REQUIRED CONFIG PATHS "{cmake_sdk_path}")

coa_add_plugin({safe_target_name}
{sources_block}
)
target_include_directories({safe_target_name} PRIVATE
{includes_block}
)
target_compile_definitions({safe_target_name} PRIVATE
  _WIN32_WINNT=0x0601
  WIN32_LEAN_AND_MEAN
  NOMINMAX
)
if(MSVC)
  # /D_USE_MATH_DEFINES= (empty, identical to Define.h's): M_PI for sources that include <cmath> before
  # any core header (Dungeon Clear documents why).
  target_compile_options({safe_target_name} PRIVATE /bigobj /D_USE_MATH_DEFINES=)
endif()
"#, safe_target_name = safe_target_name, cmake_sdk_path = cmake_sdk_path, sources_block = sources_block, includes_block = includes_block);

    write_if_changed(&staging_dir.join("CMakeLists.txt"), controlled_cmake.as_bytes())?;

    // 4. Create batch runner executing MSVC + CMake + Ninja with clean English UTF-8 output
    let ninja_dir = Path::new(&ninja).parent().unwrap_or(Path::new("")).to_string_lossy();
    let runner_bat = staging_dir.join("run_build.bat");
    let runner_content = format!(r#"@echo off
set "VSLANG=1033"
chcp 65001 > nul
call "{vcvars64}"{vcvars_ver} > nul 2>&1
if errorlevel 1 exit /b 101
set "PATH={ninja_dir};%PATH%"
"{cmake}" -G Ninja -S "{staging_dir}" -B "{build_dir}" -DCMAKE_BUILD_TYPE=RelWithDebInfo -DCoAPluginSDK_DIR="{sdk_dir}"
if errorlevel 1 exit /b 102
"{cmake}" --build "{build_dir}" -- -j{jobs}
if errorlevel 1 exit /b 103
"#, jobs = build_jobs(), vcvars64 = vcvars64, vcvars_ver = vcvars_ver, ninja_dir = ninja_dir, cmake = cmake, staging_dir = clean_path(&staging_dir), build_dir = clean_path(&build_output_dir), sdk_dir = clean_path(Path::new(&sdk_dir)));

    fs::write(&runner_bat, &runner_content)?;

    emit_progress("COMPILING", 45, "Compiling C++ sources via MSVC Cl.exe...", None);

    // 5. Execute build command silently without console popup
    let mut cmd = Command::new("cmd.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let output = cmd
        .args(["/c", &runner_bat.to_string_lossy()])
        .output();

    let (exit_code, stdout, stderr) = match output {
        Ok(out) => (
            out.status.code().unwrap_or(-1),
            decode_msvc_output(&out.stdout),
            decode_msvc_output(&out.stderr),
        ),
        Err(e) => {
            return Err(Error::Invalid(format!("[PROCESS_ERROR] Failed to run MSVC build runner: {}", e)));
        }
    };

    for line in stdout.lines() {
        all_logs.push(line.to_string());
        emit_progress("COMPILING", 65, "Building...", Some(line));
    }
    for line in stderr.lines() {
        all_logs.push(format!("[STDERR] {}", line));
        emit_progress("COMPILING", 65, "Compiler output", Some(line));
    }

    // The full log stays next to the build, so a failure can always be read afterwards.
    let _ = fs::write(staging_dir.join("build.log"), all_logs.join("
"));
    let mut errors = parse_msvc_errors(&all_logs);
    if exit_code != 0 && errors.is_empty() {
        errors.push(unparsed_failure(exit_code, &all_logs, &staging_dir.join("build.log")));
    }

    let expected_dll_path = build_output_dir.join(format!("{}.dll", safe_target_name));
    if exit_code != 0 || !expected_dll_path.is_file() {
        let duration = start_time.elapsed().as_secs_f64();
        emit_progress("FAILED", 100, "Compilation or linking failed.", None);
        return Ok(BuildResult {
            success: false,
            module_id,
            module_name: raw_module_name,
            display_name,
            version: "1.0.0".into(),
            source_commit,
            package_path: None,
            dll_path: None,
            dll_sha256: None,
            abi_fingerprint: None,
            duration_seconds: duration,
            errors,
            log_output: all_logs,
            verification_status: Some(VerificationStatus {
                l0_static: "PASS".into(),
                l1_compile: "FAIL".into(),
                l2_link: "FAIL".into(),
                l3_contract: "NOT_RUN".into(),
                l4_engine_boot: "BLOCKED".into(),
                l5_gameplay: "GAMEPLAY_NOT_TESTED".into(),
                l6_teardown: "NOT_RUN".into(),
            }),
        });
    }

    emit_progress("QUALIFYING", 80, "Verifying PE exports, architecture and ABI fingerprint...", None);

    // 6. Inspect & Qualify generated DLL
    let dll_bytes = fs::read(&expected_dll_path)?;
    let (machine, is_dll, exports, _) = parse_pe_exports(&dll_bytes)?;
    if !is_dll || machine != "AMD64 (x86_64)" {
        return Err(Error::Invalid(format!(
            "Generated artifact is not a valid 64-bit Windows PE DLL (machine: {}, is_dll: {})",
            machine, is_dll
        )));
    }

    let has_query = exports.iter().any(|e| e.decorated_name == "CoaPluginQuery" || e.decorated_name.contains("CoaPluginQuery"));
    if !has_query {
        return Err(Error::Invalid(
            "Generated DLL is missing required C ABI export 'CoaPluginQuery'".into()
        ));
    }

    let dll_sha256 = sha256_of_file(&expected_dll_path)?;
    // The fingerprint the DLL was really built with (its .coapl section) must equal the SDK's; the host
    // checks the same value against its own before loading the plugin.
    let host_abi_fingerprint = sdk_abi_fingerprint(Path::new(&sdk_dir))?;
    let dll_fingerprint = crate::provenance::coapl_fingerprint(&dll_bytes).ok_or_else(|| {
        Error::Invalid("Generated DLL has no .coapl plugin section (not built with the CoA Plugin SDK)".into())
    })?;
    if dll_fingerprint != host_abi_fingerprint {
        return Err(Error::Invalid(format!(
            "Generated DLL carries ABI fingerprint {} but the SDK is {}",
            dll_fingerprint, host_abi_fingerprint
        )));
    }

    let section = format!("COAPLUGIN1|{}|{}", module_id, dll_fingerprint);
    if !dll_bytes.windows(section.len()).any(|w| w == section.as_bytes()) {
        return Err(Error::Invalid(format!(
            "Generated DLL does not identify itself as plugin '{}' (the core would refuse it)", module_id
        )));
    }

    emit_progress("PACKAGING", 90, "Packaging plugin into CSM Plugin Store...", None);

    // 7. Package into CSM Plugin Store
    let version = "1.0.0";
    let store_dir = realm_root.join(".manager").join("plugin-store").join(&module_id).join(version);
    fs::create_dir_all(&store_dir)?;

    let target_dll_name = format!("{}.dll", module_id);
    let target_dll_path = store_dir.join(&target_dll_name);
    fs::copy(&expected_dll_path, &target_dll_path)?;

    // Copy configs if any (the bundled modules' too)
    let package_roots: Vec<&Path> = std::iter::once(module_source_dir).chain(bundled.iter().map(PathBuf::as_path)).collect();
    let conf_files: Vec<PathBuf> = package_roots.iter().flat_map(|r| collect_files_with_ext(r, "dist")).collect();
    let mut packaged_confs = Vec::new();
    for c in conf_files {
        let c_name = c.file_name().unwrap().to_string_lossy().into_owned();
        let dest = store_dir.join(&c_name);
        fs::copy(&c, &dest)?;
        packaged_confs.push(c_name);
    }

    // Copy SQL if any
    let sql_files: Vec<PathBuf> = package_roots.iter().flat_map(|r| collect_files_with_ext(r, "sql")).collect();
    let mut packaged_sqls = Vec::new();
    for s in sql_files {
        let s_name = s.file_name().unwrap().to_string_lossy().into_owned();
        let dest = store_dir.join(&s_name);
        fs::copy(&s, &dest)?;
        packaged_sqls.push(s_name);
    }

    // Write plugin.json manifest
    let manifest_content = serde_json::json!({
        "schema_version": "v0.3.2",
        "id": module_id,
        "name": display_name,
        "version": version,
        "source_commit": source_commit,
        "dll_file": target_dll_name,
        "dll_sha256": dll_sha256,
        "abi_fingerprint": host_abi_fingerprint,
        "master_loader": canonical_loader,
        "compiler": toolchain.msvc_cl_version.as_deref().map(|v| format!("MSVC {v} (x64 /MD)")),
        "built_at_iso": chrono_now_iso(),
        "configs": packaged_confs,
        "sql_files": packaged_sqls,
        "verification_status": {
            "l0_static": "PASS",
            "l1_compile": "PASS",
            "l2_link": "PASS",
            "l3_contract": "PASS",
            "l4_engine_boot": "READY",
            "l5_gameplay": "GAMEPLAY_NOT_TESTED",
            "l6_teardown": "READY"
        }
    });
    fs::write(store_dir.join("plugin.json"), serde_json::to_string_pretty(&manifest_content)?)?;
    // What to rebuild from when the core changes (crate::rebuild).
    write_build_recipe(realm_root, &module_id, &BuildRecipe {
        schema: 1,
        source_dir: module_source_dir.to_path_buf(),
        bundled: bundled.to_vec(),
        source_commit: source_commit.clone(),
    })?;

    let duration = start_time.elapsed().as_secs_f64();
    emit_progress("SUCCESS", 100, "Module built, qualified and packaged successfully.", None);

    Ok(BuildResult {
        success: true,
        module_id,
        module_name: raw_module_name,
        display_name,
        version: version.into(),
        source_commit,
        package_path: Some(store_dir.to_string_lossy().into_owned()),
        dll_path: Some(target_dll_path.to_string_lossy().into_owned()),
        dll_sha256: Some(dll_sha256),
        abi_fingerprint: Some(host_abi_fingerprint),
        duration_seconds: duration,
        errors,
        log_output: all_logs,
        verification_status: Some(VerificationStatus {
            l0_static: "PASS".into(),
            l1_compile: "PASS".into(),
            l2_link: "PASS".into(),
            l3_contract: "PASS".into(),
            l4_engine_boot: "READY".into(),
            l5_gameplay: "GAMEPLAY_NOT_TESTED".into(),
            l6_teardown: "READY".into(),
        }),
    })
}

fn chrono_now_iso() -> String {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
    format!("{}-01-01T00:00:00Z", 1970 + now / 31536000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The build engine runs one compilation at a time; the real-build tests take turns.
    static REAL_BUILDS: Mutex<()> = Mutex::new(());

    fn real_build_turn() -> std::sync::MutexGuard<'static, ()> {
        REAL_BUILDS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn test_detect_toolchain() {
        let realm = Path::new("C:/games/coa-work/plugin-spike/server-v03");
        if !realm.join("worldserver.exe").is_file() {
            return; // needs the local spike realm and its SDK (not on CI runners)
        }
        let tc = detect_toolchain_for(Some(realm));
        assert!(tc.msvc_available, "MSVC vcvars64.bat should be detected");
        assert!(tc.cmake_available, "CMake should be detected");
        assert!(tc.ninja_available, "Ninja should be detected");
        assert!(tc.sdk_available, "CoA Plugin SDK should be detected");
        assert!(tc.ready, "Complete toolchain should be ready");
    }

    #[test]
    fn realm_sdk_is_chosen_by_the_host_fingerprint() {
        let host = Path::new("C:/games/coa-work/plugin-spike/server-v03/worldserver.exe");
        let Some(host_fp) = crate::provenance::file_abi_fingerprint(host) else { return };
        let tmp = tempfile::tempdir().unwrap();
        let realm = tmp.path().join("realm");
        fs::create_dir_all(realm.join("Core")).unwrap();
        fs::copy(host, realm.join("Core/worldserver.exe")).unwrap();
        let fake_sdk = |name: &str, fp: &str| {
            let dir = tmp.path().join(name);
            fs::create_dir_all(dir.join("cmake")).unwrap();
            fs::write(dir.join("cmake/CoAPluginSDKConfig.cmake"), "# sdk").unwrap();
            fs::write(dir.join("sdk.json"), format!("{{\"abiFingerprint\":\"{}\"}}", fp)).unwrap();
            dir
        };
        assert!(realm_sdk_cmake_dir(&realm).is_none());

        // An SDK of another core is refused and changes nothing.
        let other = fake_sdk("other", &"0".repeat(64));
        let err = install_realm_sdk(&realm, &other).unwrap_err().to_string();
        assert!(err.contains("SDK_HOST_MISMATCH"), "{err}");
        assert!(realm_sdk_cmake_dir(&realm).is_none());

        // The SDK of this core is installed under its fingerprint and found by the toolchain.
        let right = fake_sdk("right", &host_fp);
        let dest = install_realm_sdk(&realm, &right).unwrap();
        assert_eq!(dest, realm_sdk_store(&realm).join(&host_fp[..16]));
        assert_eq!(realm_sdk_cmake_dir(&realm), Some(dest.join("cmake")));
        if std::env::var_os("COA_PLUGIN_SDK_DIR").is_none() {
            let tc = detect_toolchain_for(Some(&realm));
            assert_eq!(tc.sdk_abi_fingerprint.as_deref(), Some(host_fp.as_str()));
            assert!(detect_toolchain().sdk_dir.is_none(), "no SDK without a realm");
        }
    }

    #[test]
    fn bundled_sdk_zip_is_unpacked_only_for_its_own_host() {
        use std::io::Write;
        let host = Path::new("C:/games/coa-work/plugin-spike/server-v03/worldserver.exe");
        let Some(host_fp) = crate::provenance::file_abi_fingerprint(host) else { return };
        let tmp = tempfile::tempdir().unwrap();
        let realm = tmp.path().join("realm");
        fs::create_dir_all(realm.join("Core")).unwrap();
        fs::copy(host, realm.join("Core/worldserver.exe")).unwrap();
        let write_zip = |fp: &str| {
            let mut z = zip::ZipWriter::new(fs::File::create(realm.join("Core/plugin-sdk.zip")).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("sdk.json", opts).unwrap();
            z.write_all(format!("{{\"abiFingerprint\":\"{}\"}}", fp).as_bytes()).unwrap();
            // Windows PowerShell writes backslash-separated entry names.
            z.start_file("cmake\\CoAPluginSDKConfig.cmake", opts).unwrap();
            z.write_all(b"# sdk").unwrap();
            z.finish().unwrap();
        };

        write_zip(&"0".repeat(64));
        assert!(realm_sdk_cmake_dir(&realm).is_none(), "an SDK of another core must not be used");
        assert!(!realm_sdk_dir(&realm, &host_fp).exists());

        write_zip(&host_fp);
        let cmake = realm_sdk_cmake_dir(&realm).expect("the bundled SDK of this core is unpacked");
        assert_eq!(cmake, realm_sdk_dir(&realm, &host_fp).join("cmake"));
        assert!(cmake.join("CoAPluginSDKConfig.cmake").is_file());
    }

    #[test]
    fn ninja_path_length_counts_the_walk_up_from_the_build_dir() {
        let build = Path::new("C:/Users/u/AppData/Local/Temp/st/build");
        let header = Path::new("C:/games/realm/.manager/sdk/0123456789abcdef/include/a.h");
        // "C:/Users/u/AppData/Local/Temp/st/build" + "/" + "../" * 7 + "games/realm/.manager/sdk/0123456789abcdef/include/a.h"
        let expected = build.as_os_str().len() + 1 + 7 * 3 + "games/realm/.manager/sdk/0123456789abcdef/include/a.h".len();
        assert_eq!(ninja_stat_path_len(build, header), expected);
        let tmp = tempfile::tempdir().unwrap();
        let sdk = tmp.path().join("sdk");
        fs::create_dir_all(sdk.join("cmake")).unwrap();
        fs::create_dir_all(sdk.join("include").join("x".repeat(200))).unwrap();
        fs::write(sdk.join("include").join("x".repeat(200)).join("y.h"), "").unwrap();
        let err = check_dependency_path_lengths(&tmp.path().join("b"), &sdk.join("cmake")).unwrap_err().to_string();
        assert!(err.contains("BUILD_PATH_TOO_LONG"), "{err}");
    }

    #[test]
    fn test_sanitize_module_name() {
        assert_eq!(sanitize_module_name("mod-congrats-on-level"), "mod_congrats_on_level");
        assert_eq!(sanitize_module_name("my_custom-plugin!"), "my_custom_plugin");
    }

    #[test]
    fn test_parse_msvc_errors() {
        let sample = vec![
            "C:\\src\\test.cpp(42): error C2065: 'undefined_var': undeclared identifier".into(),
            "mod_test.lib(loader.obj) : error LNK2019: unresolved external symbol".into(),
        ];
        let errors = parse_msvc_errors(&sample);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].error_code, "C2065");
        assert_eq!(errors[0].line, 42);
        assert_eq!(errors[1].error_code, "LNK2019");
    }

    #[test]
    fn test_controlled_build_cohort_t01_t04() {
        let _turn = real_build_turn();
        let lab_dir = Path::new("C:/games/coa-work/module-compat-lab");
        let realm_root = Path::new("C:/games/coa-work/plugin-spike/server-v03");

        if !lab_dir.exists() || !realm_root.join("worldserver.exe").is_file() {
            return;
        }

        let modules = [
            ("mod-congrats-on-level", "congrats-on-level"),
            ("mod-npc-buffer", "npc-buffer"),
            ("mod-npc-enchanter", "npc-enchanter"),
            ("mod-reward-played-time", "reward-played-time"),
        ];

        for (folder, expected_id) in modules {
            let mod_dir = lab_dir.join(folder);
            let staging = std::env::temp_dir().join(format!("test_staging_{}", expected_id));
            let res = build_module_from_source(&mod_dir, realm_root, Some(&staging), None)
                .unwrap_or_else(|e| panic!("Build failed for {}: {}", folder, e));

            assert!(res.success, "Build of {} must succeed", folder);
            assert_eq!(res.module_id, expected_id);
            assert!(res.dll_sha256.is_some());
            assert!(res.package_path.is_some());

            let dll_p = PathBuf::from(res.dll_path.unwrap());
            assert!(dll_p.is_file(), "DLL for {} must exist", folder);

            let dll_bytes = fs::read(&dll_p).unwrap();
            let (machine, is_dll, exports, _) = parse_pe_exports(&dll_bytes).unwrap();
            assert!(is_dll);
            assert_eq!(machine, "AMD64 (x86_64)");
            assert!(exports.iter().any(|e| e.decorated_name == "CoaPluginQuery"));

            let _ = fs::remove_dir_all(&staging);
        }
    }

    #[test]
    fn test_controlled_build_ah_bot() {
        let _turn = real_build_turn();
        let lab_dir = Path::new("C:/games/coa-work/module-compat-lab");
        let realm_root = Path::new("C:/games/coa-work/plugin-spike/server-v03");

        let mod_dir = lab_dir.join("mod-ah-bot");
        if !mod_dir.exists() || !realm_root.join("worldserver.exe").is_file() {
            return;
        }

        let staging = std::env::temp_dir().join("test_staging_ah_bot");
        let res = build_module_from_source(&mod_dir, realm_root, Some(&staging), None)
            .unwrap_or_else(|e| panic!("Build failed for mod-ah-bot: {}", e));

        assert!(res.success, "Build of mod-ah-bot must succeed without linker errors. Errors: {:?}", res.errors);
        assert_eq!(res.module_id, "ah-bot");
        assert_eq!(res.display_name, "Auction House Bot");
        assert!(res.dll_sha256.is_some());
        assert!(res.package_path.is_some());

        let dll_p = PathBuf::from(res.dll_path.unwrap());
        assert!(dll_p.is_file(), "DLL for ah-bot must exist");

        let dll_bytes = fs::read(&dll_p).unwrap();
        let (machine, is_dll, exports, _) = parse_pe_exports(&dll_bytes).unwrap();
        assert!(is_dll);
        assert_eq!(machine, "AMD64 (x86_64)");
        assert!(exports.iter().any(|e| e.decorated_name == "CoaPluginQuery"));

        let _ = fs::remove_dir_all(&staging);
    }

    #[test]
    fn test_controlled_build_companions() {
        let _turn = real_build_turn();
        let repo_dir = Path::new("C:/Users/Dion/AppData/Local/CSMModulesPreview/github_cache/github/Corfirean/mod-coa-playerbots/38c02202f01708ab5600931c9e3e2ee02a33e0ed");
        let realm_root = Path::new("C:/games/coa-work/plugin-spike/server-v03");

        if !repo_dir.exists() || !realm_root.join("worldserver.exe").is_file() {
            return;
        }

        let staging = std::env::temp_dir().join("test_staging_companions");
        let res = build_module_from_source(repo_dir, realm_root, Some(&staging), None)
            .unwrap_or_else(|e| panic!("Build failed for mod-coa-playerbots: {}", e));

        assert!(res.success, "Build of companions must succeed: {:?}", res.errors);
        assert_eq!(res.module_id, "companions");
        assert!(res.dll_sha256.is_some());
        let dll_p = PathBuf::from(res.dll_path.unwrap());
        assert!(dll_p.is_file(), "mod-coa-playerbots DLL must exist");
        let _ = fs::remove_dir_all(&staging);
    }

    #[test]
    fn test_controlled_build_squid_playerbots() {
        let _turn = real_build_turn();
        let repo_dir = Path::new("C:/Users/Dion/AppData/Local/CSMModulesPreview/github_cache/github/Zyth45/mod-playerbots/3f916d45a4b55ce919bf69fee4a4ec75be12cffe");
        let realm_root = Path::new("C:/games/coa-work/plugin-spike/server-v03");

        if !repo_dir.exists() || !realm_root.join("worldserver.exe").is_file() {
            return;
        }

        let staging = std::env::temp_dir().join("test_staging_squid_playerbots");
        let res = build_module_from_source(repo_dir, realm_root, Some(&staging), None)
            .unwrap_or_else(|e| panic!("Build failed for Zyth45/mod-playerbots: {}", e));

        assert!(res.success, "Build of squid playerbots must succeed: {:?}", res.errors);
        assert_eq!(res.module_id, "playerbots");
        assert!(res.dll_sha256.is_some());
        let dll_p = PathBuf::from(res.dll_path.unwrap());
        assert!(dll_p.is_file(), "mod-playerbots DLL must exist");
        let _ = fs::remove_dir_all(&staging);
    }

    /// Dungeon Clear derives from SQUID's C++ classes, so it is bundled into the SQUID DLL. Its update
    /// must not rebuild SQUID: the second build after a one-file change has to be a small fraction of
    /// the first.
    #[test]
    fn test_controlled_build_squid_with_dungeon_clear_bundle_incremental() {
        let _turn = real_build_turn();
        let squid = Path::new("C:/Users/Dion/AppData/Local/CSMModulesPreview/github_cache/github/Zyth45/mod-playerbots/3f916d45a4b55ce919bf69fee4a4ec75be12cffe");
        let dc_lab = Path::new("C:/games/coa-work/module-compat-lab/mod-dungeon-clear/src");
        let realm_root = Path::new("C:/games/coa-work/plugin-spike/server-v03");
        if !squid.exists() || !dc_lab.exists() || !realm_root.join("worldserver.exe").is_file() {
            return;
        }

        // A private copy of Dungeon Clear, so the test can change a file.
        let dc = std::env::temp_dir().join("test_dc_src");
        let _ = fs::remove_dir_all(&dc);
        copy_tree(dc_lab, &dc);
        let staging = std::env::temp_dir().join("tsdc");
        let _ = fs::remove_dir_all(&staging);

        let started = Instant::now();
        let first = build_module_with_bundle(squid, &[dc.clone()], realm_root, Some(&staging), None)
            .unwrap_or_else(|e| panic!("bundle build failed: {}", e));
        let full = started.elapsed();
        assert!(first.success, "bundle build must succeed: {:?}", first.errors);
        let adapter = fs::read_to_string(staging.join("src").join("adapter.cpp")).unwrap();
        assert!(adapter.contains("Addmod_playerbotsScripts();") && adapter.contains("Addmod_dungeon_clearScripts();"),
            "adapter must register both modules:\n{}", adapter);
        assert!(adapter.contains("ClearQueue()"), "SQUID's queue must be cleared in onDrainBeforeDbClose");

        let loader = dc.join("mod_dungeon_clear_loader.cpp");
        let mut text = fs::read_to_string(&loader).unwrap();
        text.push_str("\n// incremental build test\n");
        fs::write(&loader, text).unwrap();

        let started = Instant::now();
        let second = build_module_with_bundle(squid, &[dc.clone()], realm_root, Some(&staging), None)
            .unwrap_or_else(|e| panic!("incremental bundle build failed: {}", e));
        let incremental = started.elapsed();
        assert!(second.success, "incremental build must succeed: {:?}", second.errors);
        eprintln!("bundle build: full {:?}, after a one-file change {:?}", full, incremental);
        assert!(incremental * 5 < full, "one-file change rebuilt too much: full {:?}, incremental {:?}", full, incremental);

        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir_all(&dc);
    }

    /// A core from CI (worldserver.exe + plugin-sdk.zip, nothing else, in COA_TEST_CI_REALM) and a PC
    /// with that core's MSVC: the manager unpacks the bundled SDK, picks the pinned toolset among the
    /// installed ones, and builds Companions, Content Scaling with its packs, and SQUID with Dungeon
    /// Clear into DLLs this worldserver.exe accepts. Sources: COA_TEST_SRC_{COMPANIONS,SCALING,SQUID,DC}.
    #[test]
    fn ci_core_builds_the_preview_modules_with_the_pinned_compiler() {
        let _turn = real_build_turn();
        let var = |k: &str| std::env::var(k).ok().map(PathBuf::from).filter(|p| p.exists());
        let Some(realm_root) = var("COA_TEST_CI_REALM") else { return };
        let tc = detect_toolchain_for(Some(&realm_root));
        eprintln!("toolchain: {:?}", tc);
        assert!(tc.ready, "{}", tc.diagnostic_message);
        assert_eq!(tc.msvc_cl_version, tc.required_cl_version, "the SDK's compiler must be chosen");
        let host_fp = crate::provenance::realm_host_fingerprint(&realm_root).unwrap();

        let mut jobs: Vec<(&str, PathBuf, Vec<PathBuf>)> = Vec::new();
        if let Some(c) = var("COA_TEST_SRC_COMPANIONS") { jobs.push(("companions", c, vec![])); }
        if let Some(s) = var("COA_TEST_SRC_SCALING") {
            let packs = ["mod-coa-tbc-content", "mod-coa-wotlk-content"].iter().map(|p| s.join("content-packs").join(p)).collect();
            jobs.push(("content-scaling", s, packs));
        }
        if let (Some(sq), Some(dc)) = (var("COA_TEST_SRC_SQUID"), var("COA_TEST_SRC_DC")) { jobs.push(("squid+dungeon-clear", sq, vec![dc])); }
        assert!(!jobs.is_empty(), "no module sources given");

        let mut failed = Vec::new();
        for (name, src, bundled) in &jobs {
            let staging = std::env::temp_dir().join(format!("tci-{}", &name[..3]));
            match build_module_with_bundle(src, bundled, &realm_root, Some(&staging), None) {
                Ok(r) if r.success && r.abi_fingerprint.as_deref() == Some(host_fp.as_str()) =>
                    eprintln!("OK   {}: {} in {:.0} s", name, r.dll_path.unwrap_or_default(), r.duration_seconds),
                Ok(r) => { eprintln!("FAIL {}: {:?}", name, r.errors); failed.push(*name); }
                Err(e) => { eprintln!("FAIL {}: {}", name, e); failed.push(*name); }
            }
        }
        eprintln!("host fingerprint {}", host_fp);
        assert!(failed.is_empty(), "failed: {:?}", failed);
    }

    #[test]
    fn cl_version_is_read_from_a_localised_banner() {
        let ru = "\u{41e}\u{43f}\u{442}\u{438}\u{43c}\u{438}\u{437}\u{438}\u{440}\u{443}\u{44e}\u{449}\u{438}\u{439} \u{43a}\u{43e}\u{43c}\u{43f}\u{438}\u{43b}\u{44f}\u{442}\u{43e}\u{440} Microsoft (R) C/C++ \u{432}\u{435}\u{440}\u{441}\u{438}\u{438} 19.44.35229 \u{434}\u{43b}\u{44f} x64";
        assert_eq!(parse_cl_version(ru).as_deref(), Some("19.44.35229"));
        assert_eq!(parse_cl_version("Microsoft (R) C/C++ Optimizing Compiler Version 19.51.36231.1 for x64").as_deref(), Some("19.51.36231"));
        assert_eq!(parse_cl_version("no version here"), None);
    }

    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).unwrap();
            }
        }
    }
}
