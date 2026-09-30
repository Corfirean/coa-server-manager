//! Build the tree of a clean base package from a source repack, without touching the repack:
//! launcher scaffolding is copied, the database comes from the repack's pristine archive (never its live data),
//! player data is removed, the characters database is rebuilt from the core's base SQL, and every migration is applied.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backup;
use crate::error::{Error, Result};
use crate::fsx;
use crate::migrations::{apply_pending, status, Status};
use crate::release::{collect_bots_sql, collect_core_sql};

pub struct Params<'a> {
    pub repack: &'a Path,
    pub core: &'a Path,
    pub bots: Option<&'a Path>,
    /// Files of the release (binaries, dist configs, addon...) laid out like a server folder; overlaid last.
    pub tree: &'a Path,
    /// Optional game data (`Data` folder) to include.
    pub data: Option<&'a Path>,
    pub out: &'a Path,
}

/// Auth tables that hold player data. Everything else (permissions, build info, realmlist) is configuration.
const AUTH_PLAYER_TABLES: [&str; 11] =
    ["account", "account_access", "account_banned", "account_muted", "account_password_history", "ip_banned", "logs", "logs_ip_actions", "realmcharacters", "uptime", "rbac_account_permissions"];

fn copy_dir(src: &Path, dst: &Path, skip: &dyn Fn(&str) -> bool, base: &Path) -> Result<u64> {
    let mut bytes = 0;
    for e in fs::read_dir(src)? {
        let e = e?;
        let p = e.path();
        let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
        if skip(&rel) {
            continue;
        }
        let ft = e.file_type()?;
        if ft.is_symlink() {
            continue;
        }
        let to = dst.join(e.file_name());
        if ft.is_dir() {
            fs::create_dir_all(&to)?;
            bytes += copy_dir(&p, &to, skip, base)?;
        } else {
            bytes += fs::copy(&p, &to)?;
        }
    }
    Ok(bytes)
}

fn seven_zip() -> Result<PathBuf> {
    for c in ["C:/Program Files/7-Zip/7z.exe", "C:/Program Files (x86)/7-Zip/7z.exe"] {
        if Path::new(c).is_file() {
            return Ok(PathBuf::from(c));
        }
    }
    Err(Error::Invalid("7-Zip is needed to read the repack's database archive (install 7-Zip).".into()))
}

/// Which repack files belong to a base package's scaffolding.
pub fn scaffolding(rel: &str) -> bool {
    let l = rel.to_lowercase();
    if l.starts_with(".state") || l.contains("__pycache__") || l.starts_with("core/") && !l.ends_with(".dll") || l.starts_with("source/") || l.starts_with("testing/") {
        return false;
    }
    if l.starts_with("bugreport/") {
        return !(l.starts_with("bugreport/logs") || l.starts_with("bugreport/reports"));
    }
    if l.starts_with("mysql/") {
        return l.starts_with("mysql/bin") || l.starts_with("mysql/lib") || l.starts_with("mysql/share");
    }
    l.starts_with("runtime/") || l.starts_with("scripts/") || l.starts_with("settings/") || l.starts_with("licenses/") || l == "readme.txt" || (l.starts_with("core/") && l.ends_with(".dll") && !l[5..].contains('/'))
}

/// A maintainer's repack may have "detailed logging" switched on: the launcher then rewrites the config templates to Trace
/// level and keeps the untouched copies as `*.template.original`. A shipped package must start quiet (an active Trace
/// log writes gigabytes and stalls the world server), so put the originals back and drop the backups.
pub fn restore_quiet_logging(settings: &Path) -> Result<()> {
    for name in ["worldserver.conf.template", "authserver.conf.template"] {
        let original = settings.join(format!("{name}.original"));
        if original.is_file() {
            fs::copy(&original, settings.join(name))?;
            fs::remove_file(&original)?;
        }
    }
    Ok(())
}

pub fn build(p: &Params, say: &dyn Fn(&str)) -> Result<()> {
    if p.out.exists() {
        return Err(Error::Invalid(format!("{} already exists; choose a new folder.", p.out.display())));
    }
    fsx::require_space(p.out, 12 * 1024 * 1024 * 1024)?;
    fs::create_dir_all(p.out)?;

    say("Copying launcher scaffolding");
    let mut top = Vec::new();
    for e in fs::read_dir(p.repack)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type()?.is_dir() {
            top.push(name);
        } else if scaffolding(&name) {
            fs::copy(e.path(), p.out.join(&name))?;
        }
    }
    for d in top {
        // Only these folders can contain scaffolding; the per-file rule decides what inside them is kept.
        if !["runtime", "scripts", "settings", "bugreport", "licenses", "mysql"].contains(&d.to_lowercase().as_str()) {
            continue;
        }
        let dst = p.out.join(&d);
        fs::create_dir_all(&dst)?;
        copy_dir(&p.repack.join(&d), &dst, &|rel| !scaffolding(rel), p.repack)?;
    }
    restore_quiet_logging(&p.out.join("Settings"))?;
    fs::create_dir_all(p.out.join("Core"))?;
    for e in fs::read_dir(p.repack.join("Core"))? {
        let e = e?;
        if e.path().extension().map(|x| x == "dll").unwrap_or(false) {
            fs::copy(e.path(), p.out.join("Core").join(e.file_name()))?;
        }
    }

    say("Extracting the pristine database archive");
    let archive = p.repack.join("mysql/data.7z");
    if !archive.is_file() {
        return Err(Error::Invalid("mysql/data.7z was not found in the repack.".into()));
    }
    let status7 = Command::new(seven_zip()?).arg("x").arg(&archive).arg(format!("-o{}", p.out.join("mysql").display())).arg("-y").output()?;
    if !status7.status.success() || !p.out.join("mysql/data/acore_world").is_dir() {
        return Err(Error::Invalid("The database archive could not be extracted.".into()));
    }
    let _ = fs::remove_file(p.out.join("mysql/data/auto.cnf")); // a new server UUID is generated on first start

    // The scratch server must not collide with anything running on this machine.
    let repack_json = p.out.join("Settings/repack.json");
    let mut cfg: serde_json::Value = fsx::read_json(&repack_json)?;
    let real_ports = cfg.clone();
    for (k, v) in [("mysqlPort", 14307u16), ("authPort", 14724), ("worldPort", 14085), ("raPort", 14443)] {
        cfg[k] = v.into();
    }
    fsx::atomic_write_json(&repack_json, &cfg)?;

    let mut sql = collect_core_sql(p.core)?;
    if let Some(b) = p.bots {
        sql.extend(collect_bots_sql(b)?);
    }
    let migrations: Vec<_> = sql.iter().map(|f| crate::manifest::Migration { id: f.id.clone(), db: f.db.clone(), sha256: f.sha256.clone(), destructive: false }).collect();
    let staging = std::env::temp_dir().join("coa-cleanbase-sql");
    let _ = fs::remove_dir_all(&staging);
    for f in &sql {
        let dst = staging.join(&f.db).join(format!("{}.sql", f.id));
        fs::create_dir_all(dst.parent().unwrap())?;
        fs::copy(&f.abs, dst)?;
    }
    // The maintainer's database records `rev_20260906_03_any_race_class.sql` as applied although its 86 race/class starts
    // are absent (the client patch offers those pairs, so creating such a character failed). The file is idempotent.
    let any_race_class = sql.iter().find(|f| f.id.contains("any_race_class")).map(|f| f.abs.clone());
    let base_chars: Vec<PathBuf> = {
        let mut v: Vec<PathBuf> = fs::read_dir(p.core.join("data/sql/base/db_characters"))
            .map_err(|_| Error::Invalid("data/sql/base/db_characters was not found in the core checkout.".into()))?
            .flatten()
            .map(|e| e.path())
            .filter(|x| x.extension().map(|e| e == "sql").unwrap_or(false))
            .collect();
        v.sort();
        v
    };

    say("Cleaning the databases");
    let out = p.out;
    backup::with_database(out, |db| {
        // 1. accounts and session data out of auth
        let existing = db.tables("acore_auth")?;
        let mut sql = String::from("SET FOREIGN_KEY_CHECKS=0;
");
        for t in AUTH_PLAYER_TABLES.iter().filter(|t| existing.iter().any(|e| e == *t)) {
            sql.push_str(&format!("TRUNCATE TABLE `acore_auth`.`{t}`;
"));
        }
        sql.push_str("SET FOREIGN_KEY_CHECKS=1;");
        db.query(&sql)?;
        // 2. characters: a brand-new schema from the core's base scripts
        db.query("DROP DATABASE IF EXISTS `acore_characters`; CREATE DATABASE `acore_characters` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci;")?;
        for f in &base_chars {
            db.run_sql_file("acore_characters", f)?;
        }
        // 3. every migration up to the core commit (already-recorded ones are skipped)
        let before = status(db, &migrations)?;
        let pending: Vec<_> = migrations.iter().zip(&before).filter(|(_, i)| i.status == Status::Pending).map(|(m, _)| m.clone()).collect();
        say(&format!("Applying {} database updates", pending.len()));
        let report = apply_pending(db, &pending, &staging, &|| Ok("(scratch database)".into()))?;
        if let Some((id, why)) = report.failed {
            return Err(Error::Invalid(format!("Database update {id} failed: {why}")));
        }
        // 3b. A module script older than a table rename re-creates the old table next to the renamed one (the core then
        // logs an error at every start). Fold its rows into the new table and drop it.
        let both = db.query("SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA='acore_world' AND TABLE_NAME IN ('item_template_ascension_compat','item_template_coa');")?;
        if both.trim() == "2" {
            db.query("INSERT IGNORE INTO acore_world.item_template_coa SELECT * FROM acore_world.item_template_ascension_compat; DROP TABLE acore_world.item_template_ascension_compat;")?;
        }
        // 3c. re-run the race/class starts when their effect is missing
        if let Some(file) = &any_race_class {
            if db.query("SELECT COUNT(*) FROM acore_world.playercreateinfo WHERE race=7 AND class=27;")?.trim() == "0" {
                db.run_sql_file("acore_world", file)?;
            }
        }
        // 4. prove it is clean
        let accounts = db.query("SELECT COUNT(*) FROM acore_auth.account;")?;
        let chars = db.query("SELECT COUNT(*) FROM acore_characters.characters;")?;
        if accounts != "0" || chars != "0" {
            return Err(Error::Invalid(format!("The database is not clean ({accounts} accounts, {chars} characters).")));
        }
        Ok(())
    })?;

    say("Removing run-time leftovers");
    for leftover in [".state", "mysql/logs", "mysql/my.ini", "mysql/admin-client.ini", "mysql/mysql.pid", "Core/Logs"] {
        let path = p.out.join(leftover);
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else if path.is_file() {
            fs::remove_file(&path)?;
        }
    }
    // Back to the shipped ports; the console account and its password are created at install time.
    let mut shipped = real_ports;
    shipped["raUsername"] = crate::db::SERVICE_ACCOUNT.into();
    shipped["raPassword"] = "".into();
    fsx::atomic_write_json(&repack_json, &shipped)?;

    say("Adding the release files");
    copy_dir(p.tree, p.out, &|_| false, p.tree)?;
    if let Some(data) = p.data {
        say("Copying game data");
        fs::create_dir_all(p.out.join("Data"))?;
        copy_dir(data, &p.out.join("Data"), &|_| false, data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffolding_keeps_the_launcher_and_drops_runtime_state_and_live_data() {
        for keep in ["Runtime/python/python.exe", "Scripts/manage.py", "Settings/worldserver.conf.template", "mysql/bin/mysqld.exe", "mysql/share/english/errmsg.sys", "BugReport/relay.py", "Core/libssl-3-x64.dll", "README.txt"] {
            assert!(scaffolding(keep), "{keep}");
        }
        for drop in [".state/world.json", "mysql/data/acore_world/x.ibd", "mysql/logs/error.log", "Core/worldserver.exe", "Core/Logs/Server.log", "BugReport/reports/a.json", "Source/server-source.zip", "Scripts/__pycache__/x.pyc", "Testing/notes.md", "mysql/data.7z"] {
            assert!(!scaffolding(drop), "{drop}");
        }
    }

    #[test]
    fn detailed_logging_templates_are_replaced_by_their_originals() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("worldserver.conf.template"), "Logger.root=6,Console Server").unwrap();
        fs::write(d.path().join("worldserver.conf.template.original"), "Logger.root=2,Console Server").unwrap();
        fs::write(d.path().join("authserver.conf.template"), "unchanged").unwrap();
        restore_quiet_logging(d.path()).unwrap();
        assert_eq!(fs::read_to_string(d.path().join("worldserver.conf.template")).unwrap(), "Logger.root=2,Console Server");
        assert!(!d.path().join("worldserver.conf.template.original").exists());
        assert_eq!(fs::read_to_string(d.path().join("authserver.conf.template")).unwrap(), "unchanged");
    }

    #[test]
    fn refuses_to_overwrite_an_existing_output_folder() {
        let d = tempfile::tempdir().unwrap();
        let p = Params { repack: d.path(), core: d.path(), bots: None, tree: d.path(), data: None, out: d.path() };
        assert!(build(&p, &|_| {}).is_err());
    }
}
