//! \[EXTENSION\] Host plug-ins (RULES.md D229, D246 to D254), each served
//! by a real process of the example plug-in
//! (`plugin/examples/plugin_example`): started before the database opens,
//! each object registered as the host's own, and the node stopped when
//! one fails. Pages and checksums in `host_plugins/pages.rs`; the log,
//! catalog, spill and file system in `host_plugins/stores.rs`.

mod pages;
mod stores;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use turborel::db::{Database, ServedConnections};
use turborel::plugin::bridge::hosted::trust::PluginServed;
use turborel::plugin::manifest::HostKind;
use turborel::value::Value;

use turborel::error::Code;

use crate::common::{Scratch, exec, rows};
use crate::plugins::fixture::{install, manifest};

/// The host objects a call or two serves, as a manifest declares them.
pub const SMALL: &str = "\
HOST ENCODING XOR (ID = 9001, OVERHEAD = 1);
HOST HASH FUNCTION FNV (ID = 9001, LENGTH = 8);
HOST CLOCK SYSTEM (EVERY = 5);
HOST KEYRING DERIVED;
HOST SERVED CONNECTIONS NOTED;
HOST BACKGROUND JOB TICK;
";

/// A scratch directory holding the host plug-in HOSTED of `objects`, its
/// directory, and the PLUGINS.HOST that names it.
pub fn hosted(test: &str, objects: &str) -> (Scratch, PathBuf, String) {
    let scratch = Scratch::new(test);
    let dir = install(&scratch.path("host"), ("HOSTED", "1.0.0"), &manifest("HOSTED", "1.0.0", "", objects));
    let spec = format!("HOSTED={}", dir.display());
    (scratch, dir, spec)
}

/// A database whose PLUGINS.HOST is `spec`, its host plug-ins started.
pub fn started(spec: &str) -> Database {
    let db = Database::new();
    db.set_setting("PLUGINS.HOST", &Value::Char(spec.to_owned())).unwrap();
    db.start_host_plugins().unwrap();
    db
}

/// Makes the plug-in in `dir` die as it does `what`, or no longer.
pub fn crash(dir: &Path, what: &str, on: bool) {
    let file = dir.join(format!("crash.{what}"));
    if on {
        std::fs::write(file, "").unwrap();
    } else {
        drop(std::fs::remove_file(file));
    }
}

/// Waits up to ten seconds for `done`.
pub fn eventually(done: impl Fn() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    done()
}

/// Verifies: [EXTENSION] RULES.md D229, D246, D247, D252 to D254: the
/// host plug-in PLUGINS.HOST names starts before the database opens, and
/// each object it serves is registered as the host's own: its encoding
/// and checksum named by the settings, its clock read by CURRENT_TIMESTAMP,
/// its keyring's keys, served connections asked, and its job run.
#[test]
fn a_host_plugins_objects_are_the_hosts_own() {
    let (_scratch, dir, spec) = hosted("host_small", SMALL);
    let db = started(&spec);
    db.set_setting("STORAGE.ENCODINGS", &Value::Char("XOR".into())).unwrap();
    db.set_setting("STORAGE.CHECKSUM", &Value::Char("FNV".into())).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    let year: i64 = rows(&s, "SELECT EXTRACT(YEAR FROM CURRENT_TIMESTAMP) FROM INFORMATION_SCHEMA.SCHEMATA FETCH FIRST 1 ROW ONLY").unwrap()[0]
        .parse()
        .unwrap();
    assert!(year >= 2026, "the program's clock, pushed: {year}");
    let rings = db.host_keyrings().unwrap();
    assert_eq!(rings.len(), 1);
    assert_eq!(rings[0].key("k1").unwrap(), rings[0].key("k1").unwrap(), "a key asked twice is the same");
    assert_ne!(rings[0].key("k1").unwrap(), rings[0].key("k2").unwrap());
    let (declared, link) = db.host_objects(HostKind::ServedConnections).unwrap().remove(0);
    let served = PluginServed::new(&declared, link);
    assert!(!served.end(7) && !served.cancel(8), "NOTED ends and cancels none");
    assert_eq!(std::fs::read_to_string(dir.join("served.log")).unwrap(), "end 7\ncancel 8\n");
    assert!(eventually(|| std::fs::read_to_string(dir.join("tick.log")).is_ok_and(|t| t.lines().count() >= 2)), "the job ran, again after");
    assert_eq!(db.host_stopped().unwrap(), None);
}

/// Verifies: [EXTENSION] RULES.md D229: a manifest declares host objects
/// or extension objects, never both; an extension's is no host plug-in's;
/// PLUGINS.HOST is the host's to set, never SQL's; and a host plug-in
/// bringing package cryptography must have its program's digest pinned.
#[test]
fn host_plugins_are_declared_by_the_host_alone() {
    let scratch = Scratch::new("host_declared");
    let mixed = install(&scratch.path("mixed"), ("MIXED", "1.0.0"), &manifest("MIXED", "1.0.0", "", "HOST CLOCK SYSTEM; FUNCTION PID() RETURNS BIGINT;"));
    let db = Database::new();
    db.set_setting("PLUGINS.HOST", &Value::Char(format!("MIXED={}", mixed.display()))).unwrap();
    let refused = db.start_host_plugins().unwrap_err();
    assert!(refused.msg.contains("D229"), "{refused:?}");
    let extension = install(&scratch.path("ext"), ("EXT", "1.0.0"), &manifest("EXT", "1.0.0", "", "FUNCTION PID() RETURNS BIGINT;"));
    let db = Database::new();
    db.set_setting("PLUGINS.HOST", &Value::Char(format!("EXT={}", extension.display()))).unwrap();
    assert!(db.start_host_plugins().unwrap_err().msg.contains("CREATE EXTENSION"));
    let crypto = install(&scratch.path("crypto"), ("CRYPTO", "1.0.0"), &manifest("CRYPTO", "1.0.0", "", "HOST PACKAGE CRYPTO DIGESTS;"));
    let db = Database::new();
    db.set_setting("PLUGINS.HOST", &Value::Char(format!("CRYPTO={}", crypto.display()))).unwrap();
    assert!(db.start_host_plugins().unwrap_err().msg.contains("pin"));
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    let s = db.session("U");
    assert_eq!(exec(&s, "SET DATABASE PLUGINS.HOST = ''"), Err(Code::Privilege));
    assert_eq!(exec(&s, "SET PLUGINS.HOST = ''"), Err(Code::Privilege));
}

/// Verifies: [EXTENSION] RULES.md D229: a host plug-in's process dying
/// stops the node: whoever asked is told why, once, and every statement
/// after is refused with SQLSTATE 38T11, until the database is opened
/// again.
#[test]
fn a_host_plugins_failure_stops_the_node() {
    let (_scratch, dir, spec) = hosted("host_stops", SMALL);
    let db = started(&spec);
    let told = Arc::new(Mutex::new(Vec::new()));
    let telling = Arc::clone(&told);
    db.on_host_stop(Arc::new(move |why: &str| telling.lock().unwrap().push(why.to_owned()))).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION U").unwrap();
    crash(&dir, "tick", true);
    assert!(eventually(|| db.host_stopped().unwrap().is_some()), "the job's process died");
    let why = db.host_stopped().unwrap().unwrap();
    assert!(why.contains("HOSTED.TICK"), "{why}");
    assert_eq!(told.lock().unwrap().clone(), vec![why]);
    let refused = db.session("U").run("VALUES 1").unwrap_err();
    assert_eq!(refused.sqlstate().as_str(), "38T11", "{refused:?}");
}
