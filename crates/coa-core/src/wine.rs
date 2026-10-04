//! Starting the game client on Linux. The client is a Windows program, so it runs under umu-launcher with a Proton build
//! (what the project's own guide uses), or under plain Wine when umu is not installed. This is the Linux counterpart of
//! starting `Ascension.exe` directly; `client::launch` hands over to it on a Linux host and Windows never comes here.
//!
//! The settings are found on their own and can all be overridden, from strongest to weakest: the environment
//! (`COA_PROTONPATH`, `COA_WINEPREFIX`), then `client-launch.json` in the Manager's folder, then what is installed.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::{Error, Result};

/// Prefix used when nothing else is said: the folder umu-launcher itself suggests for a game.
const DEFAULT_PREFIX: &str = "Games/umu/coa-client";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Runner {
    /// umu-launcher runs the game with a Proton build. `proton` is `None` when umu should pick and download one itself.
    Umu { program: PathBuf, proton: Option<PathBuf> },
    Wine { program: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub runner: Runner,
    pub prefix: PathBuf,
}

/// What to run, in a form that can be checked without running anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

/// `client-launch.json`: every key is optional.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Overrides {
    pub proton_path: Option<String>,
    pub prefix: Option<String>,
    /// "umu" or "wine": use this one even if the other is installed.
    pub runner: Option<String>,
}

/// Everything detection looks at, so it can be tested without a real computer.
pub struct Host<'a> {
    pub var: &'a dyn Fn(&str) -> Option<String>,
    pub which: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// Names of the entries of a folder (empty when it does not exist).
    pub list: &'a dyn Fn(&Path) -> Vec<String>,
    pub is_file: &'a dyn Fn(&Path) -> bool,
}

fn proton_folders(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".local/share/Steam/compatibilitytools.d"),
        home.join(".steam/root/compatibilitytools.d"),
        home.join(".steam/steam/compatibilitytools.d"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam/compatibilitytools.d"),
        PathBuf::from("/usr/share/steam/compatibilitytools.d"),
    ]
}

/// Numbers of a name in order, so that GE-Proton10-9 sorts before GE-Proton10-34.
fn version_key(name: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in name.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(cur.parse().unwrap_or(0));
            cur.clear();
        }
    }
    out
}

/// The newest GE-Proton build installed for Steam, if any. A folder only counts when it holds the `proton` script.
fn newest_proton(host: &Host, home: &Path) -> Option<PathBuf> {
    proton_folders(home)
        .into_iter()
        .flat_map(|dir| (host.list)(&dir).into_iter().filter(|n| n.starts_with("GE-Proton")).map(move |n| (n.clone(), dir.join(n))))
        .filter(|(_, path)| (host.is_file)(&path.join("proton")))
        .max_by_key(|(name, _)| version_key(name))
        .map(|(_, path)| path)
}

pub fn detect_with(host: &Host, overrides: &Overrides) -> Result<Launcher> {
    let home = (host.var)("HOME").map(PathBuf::from).ok_or_else(|| Error::Invalid("The home folder of this user is not known.".into()))?;
    let prefix = (host.var)("COA_WINEPREFIX").or_else(|| overrides.prefix.clone()).map(PathBuf::from).unwrap_or_else(|| home.join(DEFAULT_PREFIX));
    let proton = (host.var)("COA_PROTONPATH").or_else(|| overrides.proton_path.clone()).map(PathBuf::from).or_else(|| newest_proton(host, &home));
    let (umu, wine) = ((host.which)("umu-run"), (host.which)("wine"));
    let runner = match (overrides.runner.as_deref(), umu, wine) {
        (Some("wine"), _, Some(program)) => Runner::Wine { program },
        (Some("umu"), Some(program), _) | (None, Some(program), _) => Runner::Umu { program, proton },
        (_, None, Some(program)) => Runner::Wine { program },
        _ => {
            return Err(Error::Invalid(
                "To play on Linux the game client needs umu-launcher (with a Proton build such as GE-Proton) or Wine. Install one of them and press Play again.".into(),
            ))
        }
    };
    Ok(Launcher { runner, prefix })
}

/// The command line that starts `exe` from the client folder.
pub fn plan(launcher: &Launcher, client: &Path, exe: &str) -> Plan {
    let mut env = vec![("WINEPREFIX".to_string(), launcher.prefix.to_string_lossy().into_owned())];
    // DivxTac.dll is a mixed-mode .NET assembly: loaded through wine-mono it deadlocks the client on the world loading screen
    // (100%). Disabling it has the same effect as removing the file.
    env.push(("WINEDLLOVERRIDES".into(), "divxtac=d".into()));
    let program = match &launcher.runner {
        Runner::Umu { program, proton } => {
            env.push(("GAMEID".into(), "0".into()));
            if let Some(p) = proton {
                env.push(("PROTONPATH".into(), p.to_string_lossy().into_owned()));
            }
            program.clone()
        }
        Runner::Wine { program } => program.clone(),
    };
    // The client reads Data/ relative to the working directory.
    Plan { program, args: vec![exe.to_string()], env, cwd: client.to_path_buf() }
}

fn system_var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(program)).find(|p| p.is_file())
}

fn list_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default()
}

fn data_home() -> Option<PathBuf> {
    system_var("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| system_var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn overrides_file() -> Option<PathBuf> {
    data_home().map(|d| d.join("CoAServerManager/client-launch.json"))
}

pub fn detect() -> Result<Launcher> {
    let overrides: Overrides = overrides_file().and_then(|p| crate::fsx::read_json(&p).ok()).unwrap_or_default();
    detect_with(&Host { var: &system_var, which: &find_on_path, list: &list_names, is_file: &|p| p.is_file() }, &overrides)
}

/// Where what the client prints goes: the Manager's own log folder.
pub fn log_path() -> Option<PathBuf> {
    data_home().map(|d| d.join("CoAServerManager/logs/client.log"))
}

/// Start the client and return at once; it keeps running when the Manager closes. Returns the process id.
pub fn launch(client: &Path, exe: &str) -> Result<u32> {
    let launcher = detect()?;
    let plan = plan(&launcher, client, exe);
    let log = match log_path() {
        Some(p) => {
            if let Some(dir) = p.parent() {
                fs::create_dir_all(dir)?;
            }
            let mut f = OpenOptions::new().create(true).append(true).open(&p)?;
            writeln!(f, "=== {} {:?} in {}", plan.program.display(), plan.args, plan.cwd.display())?;
            for (k, v) in &plan.env {
                writeln!(f, "    {k}={v}")?;
            }
            Some(f)
        }
        None => None,
    };
    let mut cmd = Command::new(&plan.program);
    cmd.args(&plan.args).current_dir(&plan.cwd).envs(plan.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null());
    match log {
        Some(f) => {
            cmd.stderr(f.try_clone()?).stdout(f);
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // its own group: closing the Manager does not take the game with it
    }
    let mut child = cmd.spawn().map_err(|e| Error::Invalid(format!("The game client could not be started ({}): {e}", plan.program.display())))?;
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = child.wait(); // reap it when the game closes
    });
    Ok(pid)
}

/// Is a process running this client? Under Wine its command line carries the Windows form of the path
/// (`Z:\home\you\client\Ascension.exe`), which is the only trace of it that names the folder.
pub fn command_line_runs(cmdline: &str, client: &Path, exe: &str) -> bool {
    let wanted = format!("{}/{}", client.to_string_lossy().trim_end_matches('/'), exe).to_lowercase();
    cmdline.replace('\0', " ").replace('\\', "/").to_lowercase().contains(&wanted)
}

pub fn is_running(client: &Path, exes: &[&str]) -> bool {
    let client = fs::canonicalize(client).unwrap_or_else(|_| client.to_path_buf());
    let Ok(rd) = fs::read_dir("/proc") else { return false };
    rd.filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| fs::read(e.path().join("cmdline")).ok())
        .any(|raw| {
            let line = String::from_utf8_lossy(&raw);
            exes.iter().any(|exe| command_line_runs(&line, &client, exe))
        })
}

// Linux only: the folders are written as Linux path strings.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// A computer described in a few lines: variables, programs on the path, folders with their entries, and `proton` scripts.
    struct Fake {
        vars: BTreeMap<&'static str, &'static str>,
        programs: BTreeSet<&'static str>,
        folders: BTreeMap<&'static str, Vec<&'static str>>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { vars: BTreeMap::from([("HOME", "/home/ana")]), programs: BTreeSet::new(), folders: BTreeMap::new() }
        }

        fn detect(&self, overrides: &Overrides) -> Result<Launcher> {
            let var = |k: &str| self.vars.get(k).map(|v| v.to_string());
            let which = |p: &str| self.programs.contains(p).then(|| PathBuf::from(format!("/usr/bin/{p}")));
            let list = |d: &Path| self.folders.get(d.to_str().unwrap()).map(|v| v.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
            // every listed build has its `proton` script
            let is_file = |p: &Path| p.file_name().is_some_and(|n| n == "proton");
            detect_with(&Host { var: &var, which: &which, list: &list, is_file: &is_file }, overrides)
        }
    }

    const STEAM: &str = "/home/ana/.local/share/Steam/compatibilitytools.d";

    #[test]
    fn umu_with_the_newest_ge_proton_is_the_default() {
        let mut c = Fake::new();
        c.programs.extend(["umu-run", "wine"]);
        c.folders.insert(STEAM, vec!["GE-Proton9-27", "GE-Proton10-9", "GE-Proton10-34", "Proton-Experimental", "notes.txt"]);
        let l = c.detect(&Overrides::default()).unwrap();
        assert_eq!(l.prefix, PathBuf::from("/home/ana/Games/umu/coa-client"));
        match l.runner {
            Runner::Umu { proton, program } => {
                assert_eq!(program, PathBuf::from("/usr/bin/umu-run"));
                assert_eq!(proton, Some(PathBuf::from(format!("{STEAM}/GE-Proton10-34"))), "10-34 is newer than 10-9");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn without_a_proton_build_umu_is_left_to_choose_one() {
        let mut c = Fake::new();
        c.programs.insert("umu-run");
        let Runner::Umu { proton, .. } = c.detect(&Overrides::default()).unwrap().runner else { panic!() };
        assert_eq!(proton, None);
    }

    #[test]
    fn plain_wine_is_used_when_umu_is_not_installed() {
        let mut c = Fake::new();
        c.programs.insert("wine");
        assert!(matches!(c.detect(&Overrides::default()).unwrap().runner, Runner::Wine { .. }));
    }

    #[test]
    fn without_umu_or_wine_the_person_is_told_what_to_install() {
        let err = Fake::new().detect(&Overrides::default()).unwrap_err().to_string();
        assert!(err.contains("umu-launcher") && err.contains("Wine"), "{err}");
    }

    #[test]
    fn settings_override_what_is_installed_and_the_environment_overrides_the_file() {
        let mut c = Fake::new();
        c.programs.extend(["umu-run", "wine"]);
        c.folders.insert(STEAM, vec!["GE-Proton10-34"]);
        let file = Overrides { proton_path: Some("/opt/proton-a".into()), prefix: Some("/data/prefix-a".into()), runner: None };
        let l = c.detect(&file).unwrap();
        assert_eq!(l.prefix, PathBuf::from("/data/prefix-a"));
        assert!(matches!(&l.runner, Runner::Umu { proton: Some(p), .. } if p == Path::new("/opt/proton-a")));
        c.vars.insert("COA_PROTONPATH", "/opt/proton-b");
        c.vars.insert("COA_WINEPREFIX", "/data/prefix-b");
        let l = c.detect(&file).unwrap();
        assert_eq!(l.prefix, PathBuf::from("/data/prefix-b"));
        assert!(matches!(&l.runner, Runner::Umu { proton: Some(p), .. } if p == Path::new("/opt/proton-b")));
        // Forcing Wine works when it is installed, and falls back to umu when it is not.
        let wine = Overrides { runner: Some("wine".into()), ..Default::default() };
        assert!(matches!(c.detect(&wine).unwrap().runner, Runner::Wine { .. }));
    }

    #[test]
    fn the_command_is_the_one_of_the_projects_own_launcher_script() {
        let l = Launcher { runner: Runner::Umu { program: "/usr/bin/umu-run".into(), proton: Some("/p/GE-Proton11-6".into()) }, prefix: "/home/ana/Games/umu/coa-client".into() };
        let p = plan(&l, Path::new("/home/ana/CoaServer/client/ascension-live"), "Ascension.exe");
        assert_eq!(p.program, PathBuf::from("/usr/bin/umu-run"));
        assert_eq!(p.args, ["Ascension.exe"]);
        assert_eq!(p.cwd, PathBuf::from("/home/ana/CoaServer/client/ascension-live"), "the client reads Data/ from its working directory");
        let env: BTreeMap<_, _> = p.env.iter().cloned().collect();
        assert_eq!(env["GAMEID"], "0");
        assert_eq!(env["PROTONPATH"], "/p/GE-Proton11-6");
        assert_eq!(env["WINEPREFIX"], "/home/ana/Games/umu/coa-client");
        assert_eq!(env["WINEDLLOVERRIDES"], "divxtac=d", "DivxTac.dll deadlocks the world loading screen under wine-mono");
        // Wine has no GAMEID / PROTONPATH.
        let w = plan(&Launcher { runner: Runner::Wine { program: "/usr/bin/wine".into() }, prefix: "/pre".into() }, Path::new("/c"), "Wow.exe");
        assert!(w.env.iter().all(|(k, _)| k != "GAMEID" && k != "PROTONPATH") && w.args == ["Wow.exe"]);
    }

    #[test]
    fn a_wine_command_line_names_the_client_folder_in_windows_form() {
        let client = Path::new("/home/ana/CoaServer/client/ascension-live");
        assert!(command_line_runs("Z:\\home\\ana\\CoaServer\\client\\ascension-live\\Ascension.exe\0", client, "Ascension.exe"));
        assert!(command_line_runs("C:\\windows\\system32\\start.exe /exec Z:\\HOME\\ANA\\coaserver\\client\\ascension-live\\ascension.exe", client, "Ascension.exe"), "case does not matter");
        assert!(!command_line_runs("Z:\\home\\ana\\CoaServer\\client\\ascension-lab\\Ascension.exe", client, "Ascension.exe"), "another client folder");
        assert!(!command_line_runs("umu-run Ascension.exe", client, "Ascension.exe"), "the launcher alone does not name the folder");
        assert!(!command_line_runs("Z:\\home\\ana\\CoaServer\\client\\ascension-live\\Ascension.exe.ORIGINAL", Path::new("/other"), "Ascension.exe"));
    }

    #[test]
    fn newer_versions_sort_after_older_ones() {
        assert!(version_key("GE-Proton10-9") < version_key("GE-Proton10-34"));
        assert!(version_key("GE-Proton9-27") < version_key("GE-Proton10-1"));
        assert!(version_key("GE-Proton11-6-x86_64") < version_key("GE-Proton11-7-x86_64"));
    }

    #[cfg(unix)]
    #[test]
    fn a_real_launch_runs_the_program_in_the_client_folder_with_the_right_environment() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let client = d.path().join("client");
        fs::create_dir_all(&client).unwrap();
        let out = d.path().join("seen.txt");
        let script = d.path().join("umu-run");
        fs::write(&script, format!("#!/bin/sh\n{{ echo \"cwd=$(pwd)\"; echo \"arg=$1\"; echo \"prefix=$WINEPREFIX\"; echo \"gameid=$GAMEID\"; echo \"dll=$WINEDLLOVERRIDES\"; }} > '{}'\n", out.display())).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let launcher = Launcher { runner: Runner::Umu { program: script, proton: None }, prefix: d.path().join("prefix") };
        let p = plan(&launcher, &client, "Ascension.exe");
        let mut child = Command::new(&p.program).args(&p.args).current_dir(&p.cwd).envs(p.env.iter().map(|(k, v)| (k, v))).spawn().unwrap();
        assert!(child.wait().unwrap().success());
        let seen = fs::read_to_string(&out).unwrap();
        assert!(seen.contains(&format!("cwd={}", fs::canonicalize(&client).unwrap().display())), "{seen}");
        assert!(seen.contains("arg=Ascension.exe") && seen.contains("gameid=0") && seen.contains("dll=divxtac=d") && seen.contains("prefix="), "{seen}");
    }
}
