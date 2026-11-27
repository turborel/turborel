//! A transaction rolled back beside checkpoints, through a crash (RULES.md
//! D25, D470, D1250), and one whose session is dropped with no call to
//! enter (RULES.md D1259).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use crate::db::image::Image;
use crate::db::{Database, OpenOptions, Opened};
use crate::disk::io::FileSystem;
use crate::disk::memory::MemoryFileSystem;
use crate::log::{Log, Lsn, MemoryLog};
use crate::txn::Held;
use crate::value::{Decimal, Value};

/// The database in `dir` of `fs`, its log in segments of 16 KiB, no
/// checkpoint taken but those asked for.
fn open(fs: &Arc<dyn FileSystem>, dir: &Path) -> (Database, Opened) {
    let number = |n: i64| Value::Exact(Decimal::new(i128::from(n), 0));
    let db = Database::new();
    db.set_setting("LOG.SEGMENT_BYTES", &number(16 * 1024)).unwrap();
    let opened = db.open_files(fs, dir, &OpenOptions::default()).unwrap();
    db.set_setting("CHECKPOINT.AFTER_CHANGES", &number(i64::MAX)).unwrap();
    db.set_setting("CHECKPOINT.AFTER_BYTES", &number(i64::MAX)).unwrap();
    (db, opened)
}

/// The first column of the one row `sql` reads in `db`.
fn one(db: &Database, sql: &str) -> String {
    let row = db.session("APP").open_cursor(sql).unwrap().fetch().unwrap().unwrap();
    row.first().map(ToString::to_string).unwrap_or_default()
}

/// Whether the image `db`'s last checkpoint recorded has recovery read the
/// log from the position it holds every change before, so from no record
/// of a transaction open then.
fn starts_at_its_end(db: &Database) -> bool {
    let files = db.files().unwrap().unwrap();
    let image = Image::decode(&files.catalog.last().unwrap(), &db.snapshot().unwrap().types).unwrap();
    image.start == image.through
}

/// Verifies: [X3.135-1986 §8.2, §8.9]; [DESIGN] RULES.md D25, D470, D1250,
/// a transaction that updated a row and rolled back, kept after as its
/// session keeps it until the rollback's call ends, a checkpoint taken
/// then, another transaction updating the row and committing, a second
/// checkpoint, a crash: each checkpoint's image has recovery read the log
/// from its own position, no record of the transaction rolled back
/// holding it, so the second leaves the segments before it go; and
/// recovery undoes nothing and reads the row as the second transaction
/// committed it, by its key and through its index. Regression test
/// (R-TEST-7): the first checkpoint wrote the records the transaction
/// still held, with no abort record after, the log was kept from them,
/// and recovery put the row back as they found it, over the commit.
#[test]
fn a_row_rolled_back_keeps_what_was_committed_after_through_a_crash() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let dir = Path::new("/rolled-back");
    let (crashed, _) = open(&fs, dir);
    crashed.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(8), F CHAR(200))").unwrap();
    crashed.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    let s = crashed.session("APP");
    s.run("INSERT INTO T VALUES (1, 'base', 'f')").unwrap();
    s.commit().unwrap();
    let rolled = crashed.session("APP");
    rolled.run("UPDATE T SET V = 'rolled' WHERE K = 1").unwrap();
    let kept = rolled.current_transaction().unwrap();
    rolled.rollback().unwrap();
    crashed.checkpoint().unwrap();
    assert!(starts_at_its_end(&crashed), "no record of the transaction rolled back holds the log");
    s.run("UPDATE T SET V = 'kept' WHERE K = 1").unwrap();
    // Several segments of the log.
    for k in 2_u32..400_u32 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'more', 'f')")).unwrap();
    }
    s.commit().unwrap();
    crashed.checkpoint().unwrap();
    assert!(starts_at_its_end(&crashed));
    let segments = fs.read_dir(&dir.join("log")).unwrap().len();
    assert!(segments <= 2, "{segments} segments kept");
    drop((s, rolled, kept, crashed));
    let (db, opened) = open(&fs, dir);
    assert!(matches!(opened, Opened::Recovered(report) if report.undone == 0), "{opened:?}");
    assert_eq!(one(&db, "SELECT V FROM T WHERE K = 1"), "kept    ");
    assert_eq!(one(&db, "SELECT COUNT(*) FROM T WHERE V = 'kept'"), "1");
    assert_eq!(one(&db, "SELECT COUNT(*) FROM T WHERE V = 'base' OR V = 'rolled'"), "0");
}

/// Verifies: [X3.135-1986 §8.9]; [DESIGN] RULES.md D25, D1259, a session
/// dropped with no call to enter, a panic in a call's entry having left
/// it busy, its transaction open with a row updated and one inserted: its
/// transaction is rolled back as it is dropped, so the row reads as
/// committed before, another session updates it and commits, refused
/// nothing, the database takes changes still, a checkpoint's image starts
/// at its own position, and after a crash recovery undoes nothing and the
/// row reads as the second session committed it, by its key and through
/// its index, the row inserted absent. Regression test (R-TEST-7): the
/// session's drop returned with its transaction not ended, whose versions
/// stayed open in the engine with its locks gone, so the next writer of
/// the row was refused (SQLCODE -911) for the life of the process, or
/// waited for good, and a checkpoint captured them with no record of
/// them in the log.
#[test]
fn a_session_dropped_with_no_call_to_enter_ends_its_transaction() {
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let dir = Path::new("/dropped");
    let (crashed, _) = open(&fs, dir);
    crashed.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(8))").unwrap();
    crashed.create_index("T_V", "APP.T", &["V"], "btree").unwrap();
    let s = crashed.session("APP");
    s.run("INSERT INTO T VALUES (1, 'base')").unwrap();
    s.commit().unwrap();
    let lost = crashed.session("APP");
    lost.run("UPDATE T SET V = 'lost' WHERE K = 1").unwrap();
    lost.run("INSERT INTO T VALUES (3, 'lost')").unwrap();
    // A call's entry reads the session's own settings once it has marked
    // the session busy, and panics on them borrowed.
    let own = lost.own.borrow_mut();
    let entered = catch_unwind(AssertUnwindSafe(|| lost.run("SELECT K FROM T").map(drop)));
    drop(own);
    assert!(entered.is_err() && lost.busy.get(), "panicked in its entry, left busy");
    drop(lost);
    assert_eq!(one(&crashed, "SELECT V FROM T WHERE K = 1"), "base    ");
    s.run("UPDATE T SET V = 'next' WHERE K = 1").unwrap();
    s.commit().unwrap();
    assert!(crashed.log_failure().is_none());
    crashed.checkpoint().unwrap();
    assert!(starts_at_its_end(&crashed));
    drop((s, crashed));
    let (db, opened) = open(&fs, dir);
    assert!(matches!(opened, Opened::Recovered(report) if report.undone == 0), "{opened:?}");
    assert_eq!(one(&db, "SELECT V FROM T WHERE K = 1"), "next    ");
    assert_eq!(one(&db, "SELECT COUNT(*) FROM T"), "1");
    assert_eq!(one(&db, "SELECT COUNT(*) FROM T WHERE V = 'lost'"), "0");
}

/// Verifies: [DESIGN] RULES.md D57, D1259, a session dropped once its
/// database is retired, which serves no call: its transaction is rolled
/// back in memory all the same, and nothing of it is written to the log,
/// which is its replica's; taking calls again, the database reads the row
/// as committed before and takes an update of it.
#[test]
fn a_session_dropped_of_a_database_retired_ends_its_transaction() {
    let db = Database::new();
    let log: Arc<dyn Log> = Arc::new(MemoryLog::default());
    db.set_log(Arc::clone(&log)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(8))").unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T VALUES (1, 'base')").unwrap();
    s.commit().unwrap();
    let logged = log.read_from(Lsn(0)).count();
    let lost = db.session("APP");
    lost.run("UPDATE T SET V = 'lost' WHERE K = 1").unwrap();
    let coordinator = Arc::clone(&db.snapshot().unwrap().tx);
    coordinator.hold(Some(Held::Retired));
    assert!(lost.run("SELECT K FROM T").is_err(), "it serves no call");
    drop(lost);
    coordinator.hold(None);
    assert_eq!(log.read_from(Lsn(0)).count(), logged, "nothing of it logged");
    assert_eq!(one(&db, "SELECT V FROM T WHERE K = 1"), "base    ");
    s.run("UPDATE T SET V = 'next' WHERE K = 1").unwrap();
    s.commit().unwrap();
    assert_eq!(one(&db, "SELECT V FROM T WHERE K = 1"), "next    ");
}
