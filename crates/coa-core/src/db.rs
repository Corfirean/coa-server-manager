//! Database access through the MySQL client tools bundled with the repack (`mysql`, `mysqldump`, `mysqladmin`).
//!
//! Passwords travel only in the child's environment (`MYSQL_PWD`), never on a command line or in a log.

use std::fmt;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::error::{Error, ErrorCode, Result};
use crate::fsx;
use crate::layout::{read_ports, Ports};

/// A credential that never prints itself.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Account {
    /// Owner of everything in the packaged MySQL (needed for staging schemas and renames).
    Admin,
    /// The game servers' own account.
    App,
}

#[derive(Debug, Clone)]
pub struct Db {
    bin: PathBuf,
    port: u16,
    user: &'static str,
    password: Secret,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Credentials {
    root_password: String,
    app_password: String,
}

fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Schema names of the three game databases.
pub const SCHEMAS: [(&str, &str); 3] = [("characters", "acore_characters"), ("auth", "acore_auth"), ("world", "acore_world")];

pub fn schema_of(kind: &str) -> Result<&'static str> {
    SCHEMAS.iter().find(|(k, _)| *k == kind).map(|(_, s)| *s).ok_or_else(|| Error::Invalid(format!("unknown database {kind}")))
}

fn ident_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Db {
    pub fn from_repack(root: &Path, account: Account) -> Result<Db> {
        let creds: Credentials = fsx::read_json(&root.join("Settings/database.json"))
            .map_err(|_| Error::Invalid("The database settings of this server could not be read.".into()))?;
        let ports: Ports = read_ports(root);
        let bin = root.join("mysql/bin");
        if !bin.join("mysqldump.exe").is_file() || !bin.join("mysql.exe").is_file() {
            return Err(Error::Invalid("The bundled database tools are missing.".into()));
        }
        let (user, password) = match account {
            Account::Admin => ("root", creds.root_password),
            Account::App => ("acore", creds.app_password),
        };
        Ok(Db { bin, port: ports.mysql, user, password: Secret(password) })
    }

    fn command(&self, tool: &str) -> Command {
        let mut c = Command::new(self.bin.join(tool));
        c.env("MYSQL_PWD", self.password.expose())
            .arg("--protocol=tcp")
            .arg("--host=127.0.0.1")
            .arg(format!("--port={}", self.port))
            .arg(format!("--user={}", self.user))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_window(&mut c);
        c
    }

    fn fail(&self, stderr: &[u8]) -> Error {
        let text = String::from_utf8_lossy(stderr);
        let code = if text.contains("2003") || text.contains("Can't connect") { ErrorCode::DatabaseNotRunning } else { ErrorCode::Unknown };
        match code {
            ErrorCode::DatabaseNotRunning => Error::Invalid("The database is not running. Start the server first.".into()),
            _ => Error::Invalid(format!("database command failed: {}", text.trim().lines().last().unwrap_or(""))),
        }
    }

    pub fn ping(&self) -> bool {
        let mut c = self.command("mysqladmin.exe");
        c.arg("ping").arg("--connect-timeout=3");
        c.output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// Run SQL text (statement(s) supplied on stdin) and return the raw output.
    pub fn query(&self, sql: &str) -> Result<String> {
        let mut c = self.command("mysql.exe");
        c.args(["--batch", "--skip-column-names", "--connect-timeout=5"]);
        c.stdin(Stdio::piped());
        let mut child = c.spawn()?;
        child.stdin.take().expect("piped").write_all(sql.as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(self.fail(&out.stderr));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn schema_exists(&self, name: &str) -> Result<bool> {
        if !ident_ok(name) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        Ok(self.query(&format!("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name='{name}';"))? == "1")
    }

    pub fn tables(&self, schema: &str) -> Result<Vec<String>> {
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let out = self.query(&format!("SELECT table_name FROM information_schema.tables WHERE table_schema='{schema}' AND table_type='BASE TABLE' ORDER BY table_name;"))?;
        Ok(out.lines().map(str::to_string).filter(|s| !s.is_empty()).collect())
    }

    /// Bytes of data + index the schema occupies (used to estimate backup size).
    pub fn schema_bytes(&self, schema: &str) -> Result<u64> {
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let out = self.query(&format!("SELECT COALESCE(SUM(data_length+index_length),0) FROM information_schema.tables WHERE table_schema='{schema}';"))?;
        out.trim().parse().map_err(|_| Error::Invalid("unexpected size reply".into()))
    }

    /// Stored routines / triggers / views make a table-level swap unsafe; count them.
    pub fn extra_objects(&self, schema: &str) -> Result<u64> {
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let sql = format!(
            "SELECT (SELECT COUNT(*) FROM information_schema.routines WHERE routine_schema='{schema}') + (SELECT COUNT(*) FROM information_schema.triggers WHERE trigger_schema='{schema}') + (SELECT COUNT(*) FROM information_schema.views WHERE table_schema='{schema}');"
        );
        self.query(&sql)?.trim().parse().map_err(|_| Error::Invalid("unexpected reply".into()))
    }

    /// Consistent dump of one schema, zstd-compressed to `out`. Returns (compressed bytes, sha256 of the file).
    pub fn dump_to(&self, schema: &str, out: &Path) -> Result<(u64, String)> {
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let mut c = self.command("mysqldump.exe");
        c.args(["--single-transaction", "--quick", "--routines", "--triggers", "--default-character-set=utf8mb4", "--no-tablespaces", "--skip-comments", "--hex-blob"]);
        c.arg(schema);
        let mut child = c.spawn()?;
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let err_thread = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = stderr.read_to_end(&mut b);
            b
        });

        let file = File::create(out)?;
        let mut enc = zstd::Encoder::new(BufWriter::new(file), 3)?;
        let copy = std::io::copy(&mut stdout, &mut enc);
        let finish = enc.finish().and_then(|mut w| w.flush());
        let status = child.wait()?;
        let stderr_bytes = err_thread.join().unwrap_or_default();
        if !status.success() {
            let _ = std::fs::remove_file(out);
            return Err(self.fail(&stderr_bytes));
        }
        copy?;
        finish?;
        let size = std::fs::metadata(out)?.len();
        Ok((size, fsx::sha256_file(out)?))
    }

    /// Import a dump produced by `dump_to` into `schema` (which must already exist and be empty).
    pub fn import_from(&self, schema: &str, dump: &Path) -> Result<()> {
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let mut c = self.command("mysql.exe");
        c.args(["--default-character-set=utf8mb4", "--max-allowed-packet=128M"]);
        c.arg(schema);
        c.stdin(Stdio::piped());
        let mut child = c.spawn()?;
        let mut stdin = child.stdin.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let err_thread = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = stderr.read_to_end(&mut b);
            b
        });
        let mut dec = zstd::Decoder::new(BufReader::new(File::open(dump)?))?;
        let copied = std::io::copy(&mut dec, &mut stdin);
        drop(stdin);
        let status = child.wait()?;
        let stderr_bytes = err_thread.join().unwrap_or_default();
        if !status.success() {
            return Err(self.fail(&stderr_bytes));
        }
        copied?;
        Ok(())
    }
}

pub fn valid_identifier(name: &str) -> bool {
    ident_ok(name)
}
