//! The Rust bug-report relay against the repack's `BugReport/relay.py`, offline (a scripted service, no network):
//!
//! 1. the same reports and the same service answers give the same `.status` files and journal rows;
//! 2. each relay continues the other's journal: what one delivered is never sent again by the other, and a post the
//!    other left in progress is uncertain.
//!
//! usage: relay_equivalence <repack folder with Runtime/python and BugReport/relay.py>
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use coa_core::launcher::relay::{Failure, Relay, Service};

const REPORTS: [(&str, &str); 5] = [
    ("1-00000000000000a1", "COABUG1\r\nQuest broken\r\nThe NPC is gone.\r\n"),
    ("9-00000000000000b2", "COABUG1\nRate limited\nBody text\n"),
    ("3-00000000000000c3", "COABUG1\nRejected\nBody\n"),
    ("4-00000000000000d4", "COABUG1\nxy\nTitle too short\n"),
    ("5-00000000000000e5", "COABUG1\nUnknown outcome\nBody\n"),
];

/// The answers per title, in the vocabulary of both relays.
fn answer(title: &str) -> std::result::Result<u64, Failure> {
    match title {
        "Quest broken" => Ok(101),
        "Rate limited" => Err(Failure::Limited(60)),
        "Rejected" => Err(Failure::Invalid),
        _ => Err(Failure::Uncertain),
    }
}

const PYTHON: &str = r#"
import importlib.util, sys, json
from pathlib import Path
spec = importlib.util.spec_from_file_location("relay", sys.argv[1]); relay = importlib.util.module_from_spec(spec); spec.loader.exec_module(relay)
spool = Path(sys.argv[2]); calls = []
class Fake:
    def create(self, title, body):
        calls.append(title)
        if title == "Quest broken": return 101
        if title == "Rate limited": raise relay.DeliveryError("limited", 60)
        if title == "Rejected": raise relay.DeliveryError("invalid")
        raise relay.DeliveryError("uncertain")
t = [float(sys.argv[3])]
def clock():
    t[0] += 3
    return t[0]
r = relay.Relay(spool, Fake(), clock=clock)
for path in sorted(spool.glob("*.report")):
    r.process(path)
r.close()
print(json.dumps(calls))
"#;

struct Scripted(Arc<Mutex<Vec<String>>>);
impl Service for Scripted {
    fn create(&self, title: &str, _: &str) -> std::result::Result<u64, Failure> {
        self.0.lock().unwrap().push(title.into());
        answer(title)
    }
}

fn spool(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    for (key, text) in REPORTS {
        std::fs::write(dir.join(format!("{key}.report")), text).unwrap();
    }
    dir.into()
}

fn python(repack: &Path, spool: &Path, start: f64) -> Vec<String> {
    let out = Command::new(repack.join("Runtime/python/python.exe"))
        .args(["-I", "-B", "-c", PYTHON])
        .arg(repack.join("BugReport/relay.py"))
        .arg(spool)
        .arg(start.to_string())
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The clock of both relays: `start`, then 3 seconds later at every reading.
fn rust(spool: &Path, start: f64) -> Vec<String> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let t = Mutex::new(start);
    let clock = move || {
        let mut v = t.lock().unwrap();
        *v += 3.0;
        *v
    };
    let relay = Relay::with_clock(spool, Scripted(calls.clone()), Box::new(clock)).unwrap();
    relay.pass().unwrap();
    drop(relay);
    let c = calls.lock().unwrap().clone();
    c
}

/// `.status` files and journal rows (without the post time).
fn snapshot(spool: &Path) -> (Vec<(String, String)>, Vec<String>) {
    let mut statuses: Vec<(String, String)> = std::fs::read_dir(spool)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "status"))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap()))
        .collect();
    statuses.sort();
    let db = rusqlite::Connection::open(spool.join("relay.sqlite3")).unwrap();
    let mut q = db.prepare("SELECT key, digest, marker, state, number, next_try FROM deliveries ORDER BY key").unwrap();
    let rows = q
        .query_map([], |r| {
            Ok(format!(
                "{}|{}|{}|{}|{:?}|{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
                r.get::<_, f64>(5)?
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    (statuses, rows)
}

fn main() {
    let repack = PathBuf::from(std::env::args().nth(1).expect("repack folder"));
    let work = tempfile::tempdir().unwrap();

    // 1
    let (a, b) = (spool(&work.path().join("python")), spool(&work.path().join("rust")));
    let py_calls = python(&repack, &a, 1000.0);
    let rs_calls = rust(&b, 1000.0);
    println!("python posted {py_calls:?}\nrust   posted {rs_calls:?}");
    assert_eq!(py_calls, rs_calls, "the same reports are posted");
    let (pa, pb) = (snapshot(&a), snapshot(&b));
    for s in &pa.0 {
        println!("  {} = {}", s.0, s.1.trim());
    }
    assert_eq!(pa, pb, "the same statuses and journal");


    // 2: each continues the other's journal. Before the retry times nothing is sent again; after them only the
    // rate-limited report is (created, rejected and uncertain ones never are).
    assert!(python(&repack, &b, 1000.0).is_empty(), "Python does not resend what Rust handled");
    assert!(rust(&a, 1000.0).is_empty(), "Rust does not resend what Python handled");
    // Later passes: the queued reports go out, a refused or uncertain one never again.
    for start in [5000.0, 6000.0, 7000.0] {
        let (p, r) = (python(&repack, &b, start), rust(&a, start));
        println!("at {start}: python on rust's journal posted {p:?}, rust on python's posted {r:?}");
        assert_eq!(p, r, "the same retries");
        assert!(!p.iter().any(|t| t == "Quest broken" || t == "Unknown outcome"), "created and uncertain are final");
    }
    assert_eq!(snapshot(&a), snapshot(&b), "the same after a retry by the other relay");
    for dir in [&a, &b] {
        let db = rusqlite::Connection::open(dir.join("relay.sqlite3")).unwrap();
        db.execute("UPDATE deliveries SET state='posting' WHERE key='9-00000000000000b2'", []).unwrap();
    }
    python(&repack, &b, 9000.0);
    rust(&a, 9000.0);
    for dir in [&a, &b] {
        let db = rusqlite::Connection::open(dir.join("relay.sqlite3")).unwrap();
        let state: String = db.query_row("SELECT state FROM deliveries WHERE key='9-00000000000000b2'", [], |r| r.get(0)).unwrap();
        assert_eq!(state, "uncertain", "an interrupted post is not retried ({})", dir.display());
    }
    assert_eq!(snapshot(&a), snapshot(&b), "still the same after the crossover");
    println!("PASS: relay journals, statuses and posts are the same in Python and Rust");
}
