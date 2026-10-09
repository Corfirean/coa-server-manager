//! The disposable database a release is validated against, on a Linux computer.
//!
//! A release is checked by applying its database changes to the database of the signed base package. The base carries the
//! MySQL data directory of a Windows repack, and a MySQL 8.4 server in Docker opens it as it is (with the repack's table
//! name setting). So the fixture is a Docker server folder whose database container uses that directory in place of a
//! volume; everything else (start, SQL, schema capture, dumps) is the code of a Docker installation.

use std::fs;
use std::path::Path;

use super::{Config, Docker, MARKER, MYSQL_IMAGE};
use crate::driver::{self, Verb};
use crate::error::{Error, Result};
use crate::{fsx, install, package};

/// Turn an extracted base package (`mysql/data`, `Settings/`) into a Docker server folder and start its database.
pub fn prepare(root: &Path) -> Result<()> {
    prepare_with(&super::SystemDocker, root)
}

pub(crate) fn prepare_with(d: &dyn Docker, root: &Path) -> Result<()> {
    let boot = root.join(package::BOOTSTRAP_CREDENTIALS);
    if !boot.is_file() {
        return Err(Error::Invalid(
            "The package has no database bootstrap information.".into(),
        ));
    }
    let secrets: serde_json::Value = fsx::read_json(&boot)?;
    let root_pw = secrets["rootPassword"]
        .as_str()
        .filter(|p| p.chars().all(|c| c.is_ascii_hexdigit()) && !p.is_empty())
        .ok_or_else(|| Error::Invalid("The database bootstrap information is not usable.".into()))?
        .to_string();
    fs::copy(&boot, root.join("Settings/database.json"))?;
    let data = fs::canonicalize(root.join("mysql/data"))
        .map_err(|_| Error::Invalid("The package has no database data directory.".into()))?;
    let cfg = Config {
        project: format!("schema-{}", install::random_hex(8)),
        bind_address: "127.0.0.1".into(),
        mysql_image: MYSQL_IMAGE.into(),
        data_dir: None,
        mysql_data: Some(data.to_string_lossy().into_owned()),
    };
    fsx::atomic_write_json(&root.join(MARKER), &cfg)?;

    let started = driver::run(root, Verb::StartMysql)?;
    if !started.ok {
        return Err(Error::Invalid(
            started
                .human
                .map(|h| h.message.to_string())
                .unwrap_or_else(|| "The database could not be started.".into()),
        ));
    }
    // The repack's administrator account only exists for connections from the server itself (the socket); the Manager
    // connects over TCP. The fixture is thrown away, so the account is simply added.
    let sql = format!("CREATE USER IF NOT EXISTS 'root'@'127.0.0.1' IDENTIFIED BY '{root_pw}'; GRANT ALL ON *.* TO 'root'@'127.0.0.1' WITH GRANT OPTION;");
    let mut call = super::Call::new(
        &[
            "exec",
            "-i",
            "-e",
            "MYSQL_PWD",
            &cfg.names().db,
            "mysql",
            "--user=root",
        ],
        std::time::Duration::from_secs(60),
    );
    call.env = vec![("MYSQL_PWD".into(), root_pw)];
    call.stdin = Some(sql.as_bytes());
    let o = d.run(&call)?;
    if !o.ok() {
        return Err(Error::Invalid(format!(
            "The fixture database could not be opened to the Manager: {}",
            o.text()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_without_bootstrap_information_is_refused_before_anything_starts() {
        let dir = tempfile::tempdir().unwrap();
        let err = prepare(dir.path()).unwrap_err();
        assert!(err.to_string().contains("bootstrap"), "{err}");
        assert!(!dir.path().join(MARKER).exists());
    }
}
