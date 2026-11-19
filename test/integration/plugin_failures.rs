//! \[EXTENSION\] What the database does of an external plug-in that
//! fails, and of extensions across a crash (RULES.md D170 to D173): a
//! program's ERROR kept by its SQLSTATE, a crash restarted, a call that
//! never answers timed out or cancelled, malformed and overlong answers,
//! determinism, who may load an extension, the settings SQL may not set,
//! and extensions recovered and reopened.

use std::time::Duration;

use turborel::db::{Database, OpenOptions};
use turborel::error::Code;

use crate::common::{after_crash, exec, failed, number, rows};
use crate::plugins::fixture::{OBJECTS, allow, install, manifest, plugged, running};

/// The SQLSTATE of `sql`'s failure in `s`.
fn state(s: &turborel::db::Session, sql: &str) -> String {
    failed(s, sql).sqlstate().as_str().to_owned()
}

/// Verifies: [EXTENSION] RULES.md D170, D172: a program's ERROR keeps a
/// SQLSTATE of a class a routine may raise; a crash fails the call with
/// 38T01 and the next call starts another process; a frame of no message
/// is 38T03, and that process stopped too.
#[test]
fn errors_and_crashes_are_mapped_and_restarted() {
    let (_scratch, db) = plugged("plugin-failures-crash");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    assert_eq!(state(&s, "VALUES (FAIL('no'))"), "22012");
    let first = rows(&s, "VALUES (PID())").unwrap();
    assert_eq!(state(&s, "VALUES (CRASH())"), "38T01");
    let second = rows(&s, "VALUES (PID())").unwrap();
    assert_ne!(first, second, "a new process");
    assert_eq!(state(&s, "VALUES (GARBAGE())"), "38T03");
    assert_eq!(rows(&s, "VALUES (TWICE(5))").unwrap(), ["10"]);
    let failed_processes = "SELECT PROCESSES_FAILED >= 2 FROM INFORMATION_SCHEMA.EXTENSIONS";
    assert_eq!(rows(&s, failed_processes).unwrap(), ["TRUE"]);
}

/// Verifies: [EXTENSION] RULES.md D170, D44: a call longer than
/// PLUGINS.CALL_TIMEOUT_MS fails with 38T02, its process stopped; one
/// cancelled fails as a statement cancelled does; an answer longer than
/// PLUGINS.MAX_MESSAGE_BYTES fails with 38T04.
#[test]
fn timeouts_cancels_and_limits() {
    let (_scratch, db) = plugged("plugin-failures-limits");
    db.set_setting("PLUGINS.CALL_TIMEOUT_MS", &number(300)).unwrap();
    db.set_setting("PLUGINS.CANCEL_GRACE_MS", &number(50)).unwrap();
    db.set_setting("PLUGINS.MAX_MESSAGE_BYTES", &number(10_000)).unwrap();
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    assert_eq!(state(&s, "VALUES (HANG())"), "38T02");
    assert_eq!(running(&s, "EXAMPLE"), "0");
    db.set_setting("PLUGINS.CALL_TIMEOUT_MS", &number(0)).unwrap();
    let canceller = s.canceller();
    let cancelling = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        canceller.cancel();
    });
    let stopped = failed(&s, "VALUES (HANG())");
    cancelling.join().unwrap();
    assert_eq!(stopped.code, Code::Cancelled);
    assert_eq!(rows(&s, "VALUES (CHARACTER_LENGTH(HUGE(100)))").unwrap(), ["100"]);
    assert_eq!(state(&s, "VALUES (HUGE(20000))"), "38T04");
}

/// Verifies: [EXTENSION] RULES.md D169: a function its manifest says is
/// DETERMINISTIC may generate a column, one NOT DETERMINISTIC may not.
#[test]
fn determinism_is_the_manifests() {
    let (_scratch, db) = plugged("plugin-failures-determinism");
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.run("CREATE TABLE G (A INTEGER, B INTEGER GENERATED ALWAYS AS (TWICE(A)))").unwrap();
    s.run("INSERT INTO G (A) VALUES (3)").unwrap();
    assert_eq!(rows(&s, "SELECT B FROM G").unwrap(), ["6"]);
    assert!(exec(&s, "CREATE TABLE H (A INTEGER, B BIGINT GENERATED ALWAYS AS (PID()))").is_err());
}

/// Verifies: [EXTENSION] RULES.md D173: only the database's owner loads
/// or drops an extension; a directory under none of PLUGINS.DIRECTORIES
/// is refused with 38T05; SQL may not set PLUGINS.DIRECTORIES, nor write
/// MANIFEST itself.
#[test]
fn who_loads_what_from_where() {
    let (scratch, db) = plugged("plugin-failures-privileges");
    let outside = install(&scratch.path("elsewhere"), ("EXAMPLE", "1.0.0"), &manifest("EXAMPLE", "1.0.0", "", OBJECTS));
    let s = db.session("U");
    let from = s.run(&format!("CREATE EXTENSION example FROM '{}'", outside.display())).unwrap_err();
    assert_eq!(from.sqlstate().as_str(), "38T05");
    s.rollback().unwrap();
    let set = format!("SET DATABASE PLUGINS.DIRECTORIES = '{}'", outside.display());
    assert_eq!(exec(&s, &set), Err(Code::Privilege));
    let written = format!("CREATE EXTENSION example MANIFEST 'NAME example;' DIRECTORY '{}'", outside.display());
    assert_eq!(exec(&s, &written), Err(Code::SyntaxRule));
    db.create_user("OWNER", b"x").unwrap();
    db.set_owner("OWNER").unwrap();
    assert_eq!(exec(&s, "CREATE EXTENSION example"), Err(Code::Privilege));
    db.session("OWNER").run("CREATE EXTENSION example").unwrap();
    assert_eq!(rows(&s, "VALUES (TWICE(1))").unwrap(), ["2"], "any user calls it");
    assert_eq!(exec(&s, "DROP EXTENSION example"), Err(Code::Privilege));
}

/// Verifies: [EXTENSION] RULES.md D168: extensions survive a checkpoint,
/// a crash and reopen, made again of the manifest logged; one whose
/// directory has gone is still shown, and a call of it fails with 38T05;
/// one created in a transaction the crash cut is not there.
#[test]
fn extensions_survive_reopen_and_crash_recovery() {
    let (scratch, _) = plugged("plugin-failures-reopen");
    let root = scratch.path("plugins");
    install(&root, ("OTHER", "2.0.0"), &manifest("OTHER", "2.0.0", "", "FUNCTION SHOUT2(VARCHAR(200)) RETURNS VARCHAR(200);"));
    install(&root, ("LOST", "1.0.0"), &manifest("LOST", "1.0.0", "", "FUNCTION LOST_ONE(INTEGER) RETURNS INTEGER;"));
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        allow(&db, &root);
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    let s = db.session("U");
    s.run("CREATE EXTENSION example").unwrap();
    s.commit().unwrap();
    db.checkpoint().unwrap();
    s.run("CREATE EXTENSION other").unwrap();
    s.commit().unwrap();
    let crashed = db.session("V");
    crashed.run("CREATE EXTENSION lost").unwrap();
    drop(db);
    std::fs::remove_dir_all(root.join("EXAMPLE")).unwrap();
    let db = open();
    let s = db.session("U");
    let shown = "SELECT EXTENSION_NAME, VERSION FROM INFORMATION_SCHEMA.EXTENSIONS ORDER BY 1";
    assert_eq!(rows(&s, shown).unwrap(), ["EXAMPLE,1.0.0", "OTHER,2.0.0"]);
    assert_eq!(rows(&s, "VALUES (SHOUT2('back'))").unwrap(), ["BACK"]);
    assert_eq!(state(&s, "VALUES (TWICE(1))"), "38T05");
    drop(crashed);
}
