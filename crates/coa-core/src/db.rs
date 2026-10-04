//! Database access through the MySQL client tools bundled with the repack (`mysql`, `mysqldump`, `mysqladmin`), or,
//! for a Docker installation, the same tools run inside the database container with `docker exec`.
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
    realm: crate::realms::Mode,
    /// Name of the database container of a Docker installation; the client tools then run inside it.
    container: Option<String>,
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
    if kind == "playerbots" { return Ok("acore_playerbots"); }
    if let Some(kind) = kind.strip_prefix("wildcard-") { return crate::realms::Mode::Wildcard.schema(kind); }
    if let Some(kind) = kind.strip_prefix("coa-") { return crate::realms::Mode::Coa.schema(kind); }
    SCHEMAS.iter().find(|(k, _)| *k == kind).map(|(_, s)| *s).ok_or_else(|| Error::Invalid(format!("unknown database {kind}")))
}

fn ident_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// (root password, application password) from `Settings/database.json`.
pub(crate) fn credentials(root: &Path) -> Result<(String, String)> {
    let c: Credentials = fsx::read_json(&root.join("Settings/database.json"))
        .map_err(|_| Error::Invalid("The database settings of this server could not be read.".into()))?;
    Ok((c.root_password, c.app_password))
}

impl Db {
    pub fn from_repack(root: &Path, account: Account) -> Result<Db> {
        let creds: Credentials = fsx::read_json(&root.join("Settings/database.json"))
            .map_err(|_| Error::Invalid("The database settings of this server could not be read.".into()))?;
        let ports: Ports = read_ports(root);
        if crate::docker::is_docker(root) {
            return Db::in_container(root, creds, account);
        }
        let bin = root.join("mysql/bin");
        if !bin.join("mysqldump.exe").is_file() || !bin.join("mysql.exe").is_file() {
            return Err(Error::Invalid("The bundled database tools are missing.".into()));
        }
        let (user, password) = match account {
            Account::Admin => ("root", creds.root_password),
            Account::App => ("acore", creds.app_password),
        };
        Ok(Db { bin, port: ports.mysql, user, password: Secret(password), realm: crate::realms::state(root)?.active, container: None })
    }

    /// The database of a Docker installation: the client tools of the database image, run with `docker exec`. The
    /// database is not published on the host, so nothing about this depends on a port or on tools installed here.
    fn in_container(root: &Path, creds: Credentials, account: Account) -> Result<Db> {
        let container = crate::docker::Config::load(root)?.database_container();
        let (user, password) = match account {
            Account::Admin => ("root", creds.root_password),
            Account::App => ("acore", creds.app_password),
        };
        // Inside the container the server listens on its standard port; the commands below connect to it over TCP on
        // the container's own loopback, exactly like the commands of a repack do on the host.
        Ok(Db { bin: PathBuf::new(), port: 3306, user, password: Secret(password), realm: crate::realms::state(root)?.active, container: Some(container) })
    }

    pub fn for_realm(mut self, realm: crate::realms::Mode) -> Self { self.realm = realm; self }
    pub fn realm(&self) -> crate::realms::Mode { self.realm }

    pub fn realm_schema<'a>(&self, name: &'a str) -> &'a str {
        if self.realm == crate::realms::Mode::Wildcard {
            match name {
                "acore_world" => "acore_world_wildcard",
                "acore_characters" => "acore_characters_wildcard",
                _ => name,
            }
        } else { name }
    }

    pub fn clone_structure(&self, source: &str, dest: &str, cache: &Path) -> Result<()> {
        if !ident_ok(source) || !ident_ok(dest) { return Err(Error::Invalid("Invalid schema name.".into())); }
        let path = cache.join("characters-structure.sql");
        self.dump_structure_to(source, &path)?;
        self.run_sql_file(dest, &path)?;
        std::fs::remove_file(path)?;
        Ok(())
    }

    pub fn dump_structure_to(&self, source: &str, path: &Path) -> Result<()> {
        if !ident_ok(source) { return Err(Error::Invalid("Invalid schema name.".into())); }
        let mut c = self.command("mysqldump.exe");
        c.args(["--no-data", "--no-tablespaces", "--skip-comments", "--default-character-set=utf8mb4", source]);
        c.stdout(Stdio::from(File::create(&path)?));
        let out = c.output()?;
        if !out.status.success() { return Err(self.fail(&out.stderr)); }
        Ok(())
    }

    fn command(&self, tool: &str) -> Command {
        let mut c = match &self.container {
            None => Command::new(self.bin.join(tool)),
            Some(name) => {
                // `-e MYSQL_PWD` without a value takes it from this process's environment: never on a command line.
                let mut c = Command::new("docker");
                c.args(["exec", "-i", "-e", "MYSQL_PWD", name, tool.trim_end_matches(".exe")]);
                c
            }
        };
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
        let down = text.contains("2003") || text.contains("Can't connect") || text.contains("is not running") || text.contains("No such container");
        let code = if down { ErrorCode::DatabaseNotRunning } else { ErrorCode::Unknown };
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
        c.args(["--default-character-set=utf8mb4", "--max-allowed-packet=128M"]);
        c.stdin(Stdio::piped());
        let mut child = c.spawn()?;
        child.stdin.take().expect("piped").write_all(route_sql(sql, self.realm).as_bytes())?;
        let out = child.wait_with_output()?;
        if !out.status.success() {
            return Err(self.fail(&out.stderr));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn schema_exists(&self, name: &str) -> Result<bool> {
        let name = self.realm_schema(name);
        if !ident_ok(name) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        Ok(self.query(&format!("SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name='{name}';"))? == "1")
    }

    pub fn tables(&self, schema: &str) -> Result<Vec<String>> {
        let schema = self.realm_schema(schema);
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let out = self.query(&format!("SELECT table_name FROM information_schema.tables WHERE table_schema='{schema}' AND table_type='BASE TABLE' ORDER BY table_name;"))?;
        Ok(out.lines().map(str::to_string).filter(|s| !s.is_empty()).collect())
    }

    /// Bytes of data + index the schema occupies (used to estimate backup size).
    pub fn schema_bytes(&self, schema: &str) -> Result<u64> {
        let schema = self.realm_schema(schema);
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let out = self.query(&format!("SELECT COALESCE(SUM(data_length+index_length),0) FROM information_schema.tables WHERE table_schema='{schema}';"))?;
        out.trim().parse().map_err(|_| Error::Invalid("unexpected size reply".into()))
    }

    /// Stored routines / triggers / views make a table-level swap unsafe; count them.
    pub fn extra_objects(&self, schema: &str) -> Result<u64> {
        let schema = self.realm_schema(schema);
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
        let schema = self.realm_schema(schema);
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

    /// Run a plain SQL file against `schema`; the first error aborts and is returned.
    pub fn run_sql_file(&self, schema: &str, file: &Path) -> Result<()> {
        let schema = self.realm_schema(schema);
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        if self.realm == crate::realms::Mode::Wildcard {
            let sql = std::fs::read_to_string(file)?;
            self.query(&format!("SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci; USE `{schema}`;\n{sql}"))?;
            return Ok(());
        }
        let mut c = self.command("mysql.exe");
        c.args(["--default-character-set=utf8mb4", "--init-command=SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci", "--max-allowed-packet=128M"]);
        c.arg(schema);
        c.stdin(Stdio::from(File::open(file)?));
        let out = c.output()?;
        if !out.status.success() {
            return Err(self.fail(&out.stderr));
        }
        Ok(())
    }

    /// Import a dump produced by `dump_to` into `schema` (which must already exist and be empty).
    pub fn import_from(&self, schema: &str, dump: &Path) -> Result<()> {
        let schema = self.realm_schema(schema);
        if !ident_ok(schema) {
            return Err(Error::Invalid("bad schema name".into()));
        }
        let mut c = self.command("mysql.exe");
        c.args(["--default-character-set=utf8mb4", "--init-command=SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci", "--max-allowed-packet=128M"]);
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

/// Name of the service account the Manager uses for the server console (RA). Accounts are stored upper-case.
pub const SERVICE_ACCOUNT: &str = "COAMANAGER";

impl Db {
    /// Create (or reset) the console service account with a fresh random password and administrator rights on
    /// all realms, and return that password. The database must be running.
    pub fn provision_service_account(&self) -> Result<String> {
        let password = hex::encode(&crate::srp6::new_salt()[..8]); // 16 random hex characters
        let salt = crate::srp6::new_salt();
        let verifier = crate::srp6::verifier(SERVICE_ACCOUNT, &password, &salt);
        let (s, v) = (hex::encode(salt), hex::encode(verifier));
        self.query(&format!(
            "INSERT INTO acore_auth.account (username, salt, verifier, email, reg_mail) VALUES ('{SERVICE_ACCOUNT}', UNHEX('{s}'), UNHEX('{v}'), '', '')              ON DUPLICATE KEY UPDATE salt=UNHEX('{s}'), verifier=UNHEX('{v}'), failed_logins=0, locked=0;             INSERT INTO acore_auth.account_access (id, gmlevel, RealmID, comment) SELECT id, 3, -1, 'CoA Server Manager console' FROM acore_auth.account WHERE username='{SERVICE_ACCOUNT}'              ON DUPLICATE KEY UPDATE gmlevel=3;"
        ))?;
        Ok(password)
    }
}

/// Point `Settings/repack.json` at the service account (other keys are preserved).
pub fn write_console_credentials(root: &Path, password: &str) -> Result<()> {
    let path = root.join("Settings/repack.json");
    let mut v: serde_json::Value = fsx::read_json(&path)?;
    let obj = v.as_object_mut().ok_or_else(|| Error::Invalid("repack.json is not an object".into()))?;
    obj.insert("raUsername".into(), SERVICE_ACCOUNT.into());
    obj.insert("raPassword".into(), password.into());
    fsx::atomic_write_json(&path, &v)
}

pub fn valid_identifier(name: &str) -> bool {
    ident_ok(name)
}

// Route only SQL identifiers. User names, values, comments and longer identifiers remain untouched.
fn route_sql(sql: &str, realm: crate::realms::Mode) -> String {
    if realm == crate::realms::Mode::Coa { return sql.into(); }
    let bytes = sql.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if matches!(bytes[i], b'\'' | b'"') {
            let quote = bytes[i]; i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' { i = (i + 2).min(bytes.len()); }
                else if bytes[i] == quote {
                    i += 1;
                    if i < bytes.len() && bytes[i] == quote { i += 1; } else { break; }
                } else { i += 1; }
            }
            out.push_str(&sql[start..i]);
        } else if bytes[i] == b'#' || sql[i..].starts_with("--") || sql[i..].starts_with("/*") {
            if sql[i..].starts_with("/*") {
                i = sql[i + 2..].find("*/").map(|p| i + p + 4).unwrap_or(bytes.len());
            } else { i = sql[i..].find('\n').map(|p| i + p).unwrap_or(bytes.len()); }
            out.push_str(&sql[start..i]);
        } else if bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') { i += 1; }
            out.push_str(match &sql[start..i] {
                "acore_world" => "acore_world_wildcard",
                "acore_characters" => "acore_characters_wildcard",
                other => other,
            });
        } else {
            let c = sql[i..].chars().next().unwrap(); out.push(c); i += c.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod realm_tests {
    use super::*;
    #[test]
    fn routes_schema_identifiers_without_rewriting_account_values() {
        let sql = "SELECT * FROM `acore_characters`.characters WHERE name='acore_world' AND x=\"acore_characters\"; -- acore_world\nSELECT * FROM acore_world.items;";
        assert_eq!(route_sql(sql, crate::realms::Mode::Wildcard), "SELECT * FROM `acore_characters_wildcard`.characters WHERE name='acore_world' AND x=\"acore_characters\"; -- acore_world\nSELECT * FROM acore_world_wildcard.items;");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    const ROOT_PW: &str = "rootpw-ZZ1";
    const APP_PW: &str = "apppw-QQ2";

    fn folder(docker: bool) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        std::fs::create_dir_all(root.join("Settings")).unwrap();
        std::fs::write(root.join("Settings/database.json"), format!(r#"{{"rootPassword":"{ROOT_PW}","appPassword":"{APP_PW}"}}"#)).unwrap();
        if docker {
            std::fs::write(root.join("Settings/docker.json"), r#"{"project":"t1"}"#).unwrap();
        }
        (d, root)
    }

    fn args(c: &Command) -> Vec<String> {
        c.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    fn env_of(c: &Command, key: &str) -> Option<String> {
        c.get_envs().find(|(k, _)| *k == OsStr::new(key)).and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn a_docker_installation_runs_the_client_tools_inside_the_database_container() {
        let (_d, root) = folder(true);
        let db = Db::from_repack(&root, Account::Admin).unwrap();
        for (tool, program) in [("mysql.exe", "mysql"), ("mysqldump.exe", "mysqldump"), ("mysqladmin.exe", "mysqladmin")] {
            let c = db.command(tool);
            assert_eq!(c.get_program(), "docker");
            let a = args(&c);
            assert_eq!(&a[..6], ["exec", "-i", "-e", "MYSQL_PWD", "coa-t1-db", program]);
            assert!(a.contains(&"--protocol=tcp".to_string()) && a.contains(&"--host=127.0.0.1".to_string()) && a.contains(&"--port=3306".to_string()));
            assert!(a.contains(&"--user=root".to_string()));
            assert!(a.iter().all(|x| !x.contains(ROOT_PW)), "the password must not be on the command line");
            assert_eq!(env_of(&c, "MYSQL_PWD").as_deref(), Some(ROOT_PW));
        }
        let app = Db::from_repack(&root, Account::App).unwrap().command("mysql.exe");
        assert!(args(&app).contains(&"--user=acore".to_string()));
        assert_eq!(env_of(&app, "MYSQL_PWD").as_deref(), Some(APP_PW));
    }

    #[test]
    fn a_repack_still_uses_its_bundled_tools_on_the_host() {
        let (_d, root) = folder(false);
        std::fs::create_dir_all(root.join("mysql/bin")).unwrap();
        for f in ["mysql.exe", "mysqldump.exe"] {
            std::fs::write(root.join("mysql/bin").join(f), b"x").unwrap();
        }
        std::fs::write(root.join("Settings/repack.json"), r#"{"mysqlPort":3999}"#).unwrap();
        let c = Db::from_repack(&root, Account::Admin).unwrap().command("mysql.exe");
        assert_eq!(c.get_program(), root.join("mysql/bin").join("mysql.exe").as_os_str());
        let a = args(&c);
        assert!(!a.contains(&"exec".to_string()));
        assert!(a.contains(&"--port=3999".to_string()) && a.contains(&"--host=127.0.0.1".to_string()));
        assert_eq!(env_of(&c, "MYSQL_PWD").as_deref(), Some(ROOT_PW));
    }

    #[test]
    fn a_docker_installation_does_not_need_tools_in_the_server_folder() {
        let (_d, root) = folder(true);
        assert!(!root.join("mysql").exists());
        assert!(Db::from_repack(&root, Account::Admin).is_ok());
        // ...but a broken Docker settings file is reported, not ignored.
        std::fs::write(root.join("Settings/docker.json"), r#"{"project":"Bad Name"}"#).unwrap();
        assert!(Db::from_repack(&root, Account::Admin).is_err());
    }

    #[test]
    fn a_stopped_database_container_is_reported_as_a_database_that_is_not_running() {
        let (_d, root) = folder(true);
        let db = Db::from_repack(&root, Account::Admin).unwrap();
        for text in ["Error response from daemon: container abc is not running", "Error response from daemon: No such container: coa-t1-db", "ERROR 2003 (HY000): Can't connect to MySQL server"] {
            assert!(db.fail(text.as_bytes()).to_string().contains("not running"), "{text}");
        }
        assert!(db.fail(b"ERROR 1045 (28000): Access denied for user 'root'").to_string().contains("Access denied"));
    }
}
