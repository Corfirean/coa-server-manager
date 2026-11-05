//! Installs the C++ compiler that plugins must be built with.
//!
//! A plugin loads only into a core built by the same MSVC, to the build number (it is part of the ABI
//! fingerprint). The core is built with the MSVC 14.44 toolset of the stable Visual Studio channel, so
//! the Manager installs exactly that: Microsoft's own Build Tools bootstrapper, downloaded from Microsoft
//! and checked to be signed by Microsoft, with the components preselected. The person accepts Microsoft's
//! terms in Microsoft's installer window; nothing is bundled or installed silently.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::build_engine::{detect_toolchain_for, ToolchainStatus};
use crate::error::{Error, Result};

/// The stable-channel Build Tools bootstrapper (Visual Studio 18). Its MSVC 14.44 side-by-side toolset has
/// the same compiler build as the CI image the core is built on.
pub const BOOTSTRAPPER_URL: &str = "https://aka.ms/vs/18/stable/vs_buildtools.exe";

/// The toolset alone has no environment script; the core build tools and the Windows SDK complete it.
pub const COMPONENTS: [&str; 4] = [
    "Microsoft.VisualStudio.Component.VC.14.44.17.14.x86.x64",
    "Microsoft.VisualStudio.Component.VC.CoreBuildTools",
    "Microsoft.VisualStudio.Component.Windows11SDK.26100",
    "Microsoft.VisualStudio.Component.VC.CMake.Project",
];

#[derive(Debug, Clone, Serialize)]
pub struct MsvcSetupOutcome {
    /// The installer finished and the toolchain now matches the core.
    pub ready: bool,
    pub restart_required: bool,
    pub message: String,
    pub toolchain: ToolchainStatus,
}

/// An installed Visual Studio 18 Build Tools instance, which the installer modifies instead of adding a
/// second one.
fn existing_build_tools() -> Option<PathBuf> {
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:\\Program Files (x86)".into());
    let vswhere = Path::new(&pf86).join("Microsoft Visual Studio").join("Installer").join("vswhere.exe");
    let mut cmd = Command::new(vswhere);
    cmd.args(["-products", "Microsoft.VisualStudio.Product.BuildTools", "-version", "[18.0,19.0)",
        "-property", "installationPath", "-utf8"]);
    hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().map(str::trim).find(|l| !l.is_empty()).map(PathBuf::from)
        .filter(|p| p.is_dir())
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

const SCRIPT: &str = r#"param([string]$Dir, [string]$Url, [string]$InstallPath, [string]$Components)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
$bt = Join-Path $Dir 'vs_buildtools.exe'
try { Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $bt } catch { "RESULT=download-failed $($_.Exception.Message)"; exit 0 }
$sig = Get-AuthenticodeSignature -FilePath $bt
if ($sig.Status -ne 'Valid' -or $sig.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation') {
  Remove-Item $bt -Force
  "RESULT=bad-signature $($sig.Status)"; exit 0
}
$a = @()
if ($InstallPath) { $a += @('modify', '--installPath', ('"' + $InstallPath + '"')) }
$a += @('--wait', '--norestart', '--nocache')
foreach ($c in $Components.Split(',')) { $a += @('--add', $c) }
try { $p = Start-Process -FilePath $bt -ArgumentList $a -Wait -PassThru } catch { "RESULT=not-started $($_.Exception.Message)"; exit 0 }
"RESULT=exit $($p.ExitCode)"
"#;

/// Runs Microsoft's installer and reports whether plugins can now be built for `realm_root`'s core.
/// Blocks until the installer window is closed.
pub fn install_build_tools(realm_root: Option<&Path>) -> Result<MsvcSetupOutcome> {
    if !cfg!(windows) {
        return Err(Error::Invalid("The C++ Build Tools installer is only available on Windows.".into()));
    }
    let dir = std::env::temp_dir().join("coa-msvc-setup");
    std::fs::create_dir_all(&dir)?;
    let script = dir.join("install-build-tools.ps1");
    std::fs::write(&script, SCRIPT)?;
    let install_path = existing_build_tools().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .args(["-Dir"]).arg(dir.join("bootstrapper"))
        .args(["-Url", BOOTSTRAPPER_URL, "-InstallPath", &install_path, "-Components", &COMPONENTS.join(",")]);
    hide_window(&mut cmd);
    let out = cmd.output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let result = text.lines().find_map(|l| l.trim().strip_prefix("RESULT=")).unwrap_or("").to_string();
    let (installed, restart, message) = interpret(&result, &String::from_utf8_lossy(&out.stderr));
    let toolchain = detect_toolchain_for(realm_root);
    let ready = installed && toolchain.ready;
    let message = if installed && !toolchain.ready {
        format!("The installer finished, but the compiler still does not match this core: {}", toolchain.diagnostic_message)
    } else {
        message
    };
    Ok(MsvcSetupOutcome { ready, restart_required: restart, message, toolchain })
}

/// (installed, restart required, message) for the script's result line.
fn interpret(result: &str, stderr: &str) -> (bool, bool, String) {
    let (kind, detail) = result.split_once(' ').unwrap_or((result, ""));
    match (kind, detail.trim()) {
        ("exit", "0") => (true, false, "Visual Studio Build Tools with MSVC 14.44 are installed.".into()),
        ("exit", "3010") => (true, true, "Visual Studio Build Tools are installed; Windows needs a restart before modules can be built.".into()),
        ("exit", "1602") | ("exit", "1223") => (false, false, "The installation was cancelled.".into()),
        ("exit", "8006") => (false, false, "Close Visual Studio and its installer, then try again.".into()),
        ("exit", code) => (false, false, format!("The Visual Studio installer stopped with code {code}.")),
        ("bad-signature", s) => (false, false, format!("The downloaded installer is not signed by Microsoft ({s}); it was deleted and not run.")),
        ("download-failed", why) => (false, false, format!("The Visual Studio installer could not be downloaded: {why}")),
        ("not-started", why) => (false, false, format!("The Visual Studio installer could not be started: {why}")),
        _ => (false, false, format!("The installer script failed: {}", stderr.lines().next().unwrap_or("no output"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_results_are_explained() {
        assert!(interpret("exit 0", "").0);
        assert_eq!(interpret("exit 3010", "").1, true);
        assert!(!interpret("exit 1602", "").0);
        assert!(interpret("bad-signature NotSigned", "").2.contains("not signed by Microsoft"));
        assert!(interpret("", "boom").2.contains("boom"));
    }

    #[test]
    fn the_components_name_the_pinned_toolset() {
        assert!(COMPONENTS.iter().any(|c| c.contains("VC.14.44")));
        assert!(BOOTSTRAPPER_URL.starts_with("https://aka.ms/vs/18/stable/"));
    }
}
