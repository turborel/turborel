//! The anomaly matrix [ISO/IEC 9075-2:2023 §4.41.2, Table 8] (RULES.md
//! D29, D440, D441): for each isolation level, which anomalies a pair of
//! transactions can show, over a database whose log syncs slowly, so each
//! commit releases its locks long before it is durable. Dirty reads and
//! lost updates never show; nonrepeatable reads, read skew and phantoms
//! show only below REPEATABLE READ; a lost update by a read and a later
//! write shows only at READ COMMITTED; write skew shows below SERIALIZABLE.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use turborel::db::{Database, Session};
use turborel::disk::io::Durability;
use turborel::error::{Code, Result, SqlError};
use turborel::log::{Log, LogCapabilities, Lsn, MemoryLog, Records};
use turborel::txn::LockWait;

use crate::common::{exec, rows};

/// A memory log each of whose syncs takes `pause` (RULES.md D6, a
/// test-local plug-in): a commit's sync is long beside its work. Its syncs
/// fail while `failing`.
pub struct Slow {
    pub pause: Duration,
    pub log: MemoryLog,
    pub failing: AtomicBool,
}

impl Slow {
    /// It, its syncs taking `pause`.
    pub fn new(pause: Duration) -> Slow {
        Slow { pause, log: MemoryLog::new(), failing: AtomicBool::new(false) }
    }
}

impl Log for Slow {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: true }
    }
    fn set_durability(&self, durability: Durability) {
        self.log.set_durability(durability);
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        self.log.append(record)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        thread::sleep(self.pause);
        if self.failing.load(Ordering::SeqCst) {
            return Err(SqlError::new(Code::Io, "injected: the log cannot sync"));
        }
        self.log.sync(through)
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        self.log.read_from(from)
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        self.log.truncate_before(before)
    }
    fn end(&self) -> Lsn {
        self.log.end()
    }

    fn bytes_between(&self, from: Lsn, to: Lsn) -> u64 {
        self.log.bytes_between(from, to)
    }
}

/// The levels, weakest first.
const LEVELS: [&str; 4] = ["READ UNCOMMITTED", "READ COMMITTED", "REPEATABLE READ", "SERIALIZABLE"];

/// A database of `T (K, V)` holding (1, 10) and (2, 20), whose log syncs
/// slowly and whose sessions wait for locks.
fn database() -> Database {
    let db = Database::new();
    db.set_log(Arc::new(Slow::new(Duration::from_millis(5)))).unwrap();
    db.set_lock_wait(LockWait::Wait).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL PRIMARY KEY, V INTEGER)").unwrap();
    let s = db.session("A");
    s.run("INSERT INTO T VALUES (1, 10), (2, 20)").unwrap();
    s.commit().unwrap();
    db
}

/// A session of `db` whose next transaction is at `level`.
fn at_level(db: &Database, level: &str) -> Session {
    let s = db.session("A");
    s.run(&format!("SET TRANSACTION ISOLATION LEVEL {level}")).unwrap();
    s
}

/// What `s` reads of `sql`, its one value.
fn one(s: &Session, sql: &str) -> String {
    rows(s, sql).unwrap().concat()
}

/// P1: whether `level` read a change not committed.
fn dirty_read(db: &Database, level: &str) -> bool {
    let (s, other) = (at_level(db, level), db.session("A"));
    other.run("UPDATE T SET V = 11 WHERE K = 1").unwrap();
    let read = one(&s, "SELECT V FROM T WHERE K = 1");
    other.rollback().unwrap();
    s.commit().unwrap();
    read != "10"
}

/// P2: whether `level` read a row again and found it changed.
fn nonrepeatable_read(db: &Database, level: &str) -> bool {
    let (s, other) = (at_level(db, level), db.session("A"));
    let first = one(&s, "SELECT V FROM T WHERE K = 1");
    other.run("UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
    other.commit().unwrap();
    let again = one(&s, "SELECT V FROM T WHERE K = 1");
    s.commit().unwrap();
    first != again
}

/// P3: whether `level` read a condition again and found another row.
fn phantom(db: &Database, level: &str) -> bool {
    let (s, other) = (at_level(db, level), db.session("A"));
    let first = one(&s, "SELECT COUNT(*) FROM T WHERE V > 0");
    other.run("INSERT INTO T VALUES (3, 30)").unwrap();
    other.commit().unwrap();
    let again = one(&s, "SELECT COUNT(*) FROM T WHERE V > 0");
    s.commit().unwrap();
    first != again
}

/// Whether `level` read one row before a commit that moved value between
/// two, and the other after it: a sum that never was.
fn read_skew(db: &Database, level: &str) -> bool {
    let (s, other) = (at_level(db, level), db.session("A"));
    let first: i64 = one(&s, "SELECT V FROM T WHERE K = 1").parse().unwrap();
    other.run("UPDATE T SET V = V - 5 WHERE K = 1").unwrap();
    other.run("UPDATE T SET V = V + 5 WHERE K = 2").unwrap();
    other.commit().unwrap();
    let second: i64 = one(&s, "SELECT V FROM T WHERE K = 2").parse().unwrap();
    s.commit().unwrap();
    first + second != 30
}

/// Whether an increment by a statement of `level`, waiting for another's
/// on the same row, lost it: the second waits, then writes over the first
/// once it commits, or is refused (SQLCODE -911) and runs again.
fn lost_update(db: &Database, level: &str) -> bool {
    let (s, other) = (at_level(db, level), db.session("A"));
    assert_eq!(one(&s, "SELECT V FROM T WHERE K = 2"), "20");
    other.run("UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
    let waiting = thread::spawn(move || {
        let first = exec(&s, "UPDATE T SET V = V + 1 WHERE K = 1").map(drop);
        if first.is_ok() {
            s.commit().unwrap();
        } else {
            assert_eq!(first, Err(Code::Conflict));
            s.rollback().unwrap();
            exec(&s, "UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
            s.commit().unwrap();
        }
    });
    thread::sleep(Duration::from_millis(20));
    other.commit().unwrap();
    waiting.join().unwrap();
    one(&other, "SELECT V FROM T WHERE K = 1") != "12"
}

/// Whether `level` let a value read, and written back changed by a later
/// statement, overwrite a commit made between them.
fn lost_update_by_read(db: &Database, level: &str) -> bool {
    let (s, other) = (at_level(db, level), db.session("A"));
    let read: i64 = one(&s, "SELECT V FROM T WHERE K = 1").parse().unwrap();
    other.run("UPDATE T SET V = V + 1 WHERE K = 1").unwrap();
    other.commit().unwrap();
    let wrote = exec(&s, &format!("UPDATE T SET V = {} WHERE K = 1", read + 1)).is_ok() && s.commit().is_ok();
    if !wrote {
        s.rollback().unwrap();
    }
    one(&other, "SELECT V FROM T WHERE K = 1") != "12" && wrote
}

/// A5B: whether two transactions of `level`, each reading both rows and
/// changing one, both committed.
fn write_skew(db: &Database, level: &str) -> bool {
    let (s, other) = (at_level(db, level), at_level(db, level));
    assert_eq!(one(&s, "SELECT SUM(V) FROM T"), "30");
    assert_eq!(one(&other, "SELECT SUM(V) FROM T"), "30");
    s.run("UPDATE T SET V = V - 10 WHERE K = 1").unwrap();
    other.run("UPDATE T SET V = V - 20 WHERE K = 2").unwrap();
    s.commit().unwrap();
    match other.commit() {
        Ok(()) => true,
        Err(e) => {
            assert_eq!(e.code, Code::Conflict, "{level}");
            false
        }
    }
}

/// An anomaly, and the levels it shows at, as `LEVELS` lists them; none
/// for a level it does not run at (READ UNCOMMITTED is READ ONLY).
type Row = (&'static str, fn(&Database, &str) -> bool, [Option<bool>; 4]);

/// The matrix.
const MATRIX: [Row; 7] = [
    ("dirty read", dirty_read, [Some(false), Some(false), Some(false), Some(false)]),
    ("nonrepeatable read", nonrepeatable_read, [Some(true), Some(true), Some(false), Some(false)]),
    ("phantom", phantom, [Some(true), Some(true), Some(false), Some(false)]),
    ("read skew", read_skew, [Some(true), Some(true), Some(false), Some(false)]),
    ("lost update", lost_update, [None, Some(false), Some(false), Some(false)]),
    ("lost update by a read", lost_update_by_read, [None, Some(true), Some(false), Some(false)]),
    ("write skew", write_skew, [None, Some(true), Some(true), Some(false)]),
];

/// Verifies: [ISO/IEC 9075-2:2023 §4.41.2, Table 8; ISO/IEC 9075:1992
/// §4.28]; [DESIGN] RULES.md D29, D441: each anomaly shows at exactly the
/// levels the matrix says, every one the standard lets show, and
/// SERIALIZABLE shows none, while every commit releases its locks before
/// its slow sync.
#[test]
fn each_level_shows_only_its_anomalies() {
    for (anomaly, shows, expected) in MATRIX {
        for (level, expected) in LEVELS.into_iter().zip(expected) {
            let Some(expected) = expected else { continue };
            assert_eq!(shows(&database(), level), expected, "{anomaly} at {level}");
        }
    }
}
