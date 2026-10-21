//! The database of a Docker server has no log file in the server folder: MySQL writes to its container's output.

use super::cli::{Call, Docker, SystemDocker};
use super::Config;
use crate::console::Line;
use crate::error::{Error, Result};
use std::path::Path;
use std::time::Duration;

/// The last lines MySQL wrote (what `docker logs` returns), filtered and classified like the lines of a log file.
pub fn database_log(root: &Path, filter: Option<&str>, max_lines: usize) -> Result<Vec<Line>> {
    database_log_with(&SystemDocker, root, filter, max_lines)
}

pub fn database_log_with(d: &dyn Docker, root: &Path, filter: Option<&str>, max_lines: usize) -> Result<Vec<Line>> {
    let name = Config::load(root)?.names().db;
    // Ask for more than wanted: the filter may drop most of them.
    let tail = (max_lines * 4).clamp(200, 8000).to_string();
    let out = d.run(&Call::new(&["logs", "--tail", &tail, &name], Duration::from_secs(20)))?;
    if !out.ok() {
        return Err(Error::Invalid("This log does not exist yet.".into()));
    }
    // MySQL writes to the error stream.
    Ok(crate::console::lines_from_text(&format!("{}\n{}", out.stderr, out.stdout), filter, max_lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::cli::Output;
    use std::cell::RefCell;

    struct Fake {
        out: Output,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl Docker for Fake {
        fn run(&self, call: &Call) -> Result<Output> {
            self.calls.borrow_mut().push(call.args.clone());
            Ok(self.out.clone())
        }
    }

    fn root() -> (tempfile::TempDir, std::path::PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        std::fs::create_dir_all(root.join("Settings")).unwrap();
        std::fs::write(root.join("Settings/docker.json"), r#"{"project":"t1"}"#).unwrap();
        (d, root)
    }

    #[test]
    fn the_database_log_is_what_the_container_printed() {
        let (_d, root) = root();
        let fake = Fake { out: Output { code: Some(0), stdout: "2026-10-04T10:00:00Z [System] ready\n".into(), stderr: "2026-10-04T10:00:01Z [ERROR] [MY-010] boom\n2026-10-04T10:00:02Z [Warning] slow\n".into() }, calls: Default::default() };
        let lines = database_log_with(&fake, &root, None, 100).unwrap();
        assert_eq!(fake.calls.borrow()[0][..2], ["logs", "--tail"]);
        assert_eq!(fake.calls.borrow()[0].last().unwrap(), "coa-t1-db");
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().any(|l| l.text.contains("boom") && l.level == crate::console::Level::Error));
        let only = database_log_with(&fake, &root, Some("slow"), 100).unwrap();
        assert_eq!(only.len(), 1);
    }

    #[test]
    fn a_database_that_never_ran_has_no_log_yet() {
        let (_d, root) = root();
        let fake = Fake { out: Output { code: Some(1), stdout: String::new(), stderr: "Error: No such container: coa-t1-db".into() }, calls: Default::default() };
        assert!(database_log_with(&fake, &root, None, 10).unwrap_err().to_string().contains("does not exist yet"));
    }
}
