//! Index-only scans (RULES.md D34): a scan of an index whose key holds
//! every column a query reads, each restored from it, gives a row of its
//! zone's from the entry once maintenance settles the zone visible to
//! every reader, and reads it otherwise: under writes, open snapshots,
//! deletes, recovery and index entries collection could not take back.
//! The index method of these tests says a false key for one row: a row
//! read refuses it, a row made of its entry shows it, so each query shows
//! which way its rows were made.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use turborel::db::{Database, OpenOptions, Session};
use turborel::error::{Code, Result, SqlError};
use turborel::index::{BTreeMethod, Entries, Index, IndexMethod, Key, KeyOrder, Probe};
use turborel::params::Params;
use turborel::storage::RowId;
use turborel::threads::JobPolicy;
use turborel::value::{CompOp, DataType};

use crate::common::{Scratch, number, os, rows, sorted};

/// Rows of table T: three zones of row ids, the last in part.
const ROWS: i32 = 2_100;

/// What the tests tell their index method: to fail each removal, and to
/// say row K = 1, V = 1's key is K = 1, V = 1001.
#[derive(Default)]
struct Flags {
    fail: AtomicBool,
    lie: AtomicBool,
}

/// The B-link tree's method, its keys restored, told by `Flags`.
struct Probing {
    inner: BTreeMethod,
    flags: Arc<Flags>,
}

/// An index of `Probing`.
struct ProbingIndex {
    inner: Arc<dyn Index>,
    flags: Arc<Flags>,
}

impl IndexMethod for Probing {
    fn name(&self) -> &'static str {
        "probing"
    }
    fn supports(&self, op: CompOp) -> bool {
        self.inner.supports(op)
    }
    fn supports_prefix(&self, columns: usize, equal: usize, then: Option<CompOp>) -> bool {
        self.inner.supports_prefix(columns, equal, then)
    }
    fn supports_range(&self, columns: usize, equal: usize) -> bool {
        self.inner.supports_range(columns, equal)
    }
    fn ordered(&self) -> bool {
        true
    }
    fn restores_keys(&self) -> bool {
        true
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        Ok(Box::new(ProbingIndex { inner: Arc::from(self.inner.create(columns)?), flags: Arc::clone(&self.flags) }))
    }
}

impl Index for ProbingIndex {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.inner.insert(key, row)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        if self.flags.fail.load(Ordering::SeqCst) {
            return Err(SqlError::new(Code::Io, "a removal the test fails"));
        }
        self.inner.remove(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.inner.rows(key)
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.inner.estimate(probe)
    }
    fn search(self: Arc<Self>, probe: &Probe, order: KeyOrder) -> Entries {
        let entries = Arc::clone(&self.inner).search(probe, order);
        let truth = Key(vec![number(1), number(1)]);
        Box::new(entries.map(move |entry| {
            entry.map(|(key, row)| {
                let lie = self.flags.lie.load(Ordering::SeqCst) && key == truth;
                (if lie { Key(vec![number(1), number(1_001)]) } else { key }, row)
            })
        }))
    }
}

/// A database of JOBS.POLICY MANUAL, the method `Probing` told by
/// `flags` registered.
fn database(flags: &Arc<Flags>) -> Database {
    let db = Database::new();
    db.set_job_policy(JobPolicy::Manual).unwrap();
    db.register_index_method(Box::new(Probing { inner: BTreeMethod::in_memory(), flags: Arc::clone(flags) })).unwrap();
    db
}

/// Schema X of `db`: table T (K, V, W VARCHAR, X DOUBLE PRECISION), and
/// S, of the same rows, of no index; K 0 to `ROWS` - 1, V K, W 'w' and K,
/// X K / 10; T's index T_KV on (K, V) of `Probing`.
fn filled(db: &Database) -> Session {
    let columns = "K INTEGER NOT NULL, V INTEGER NOT NULL, W VARCHAR(8), X DOUBLE PRECISION";
    db.schema(&format!("CREATE SCHEMA AUTHORIZATION X CREATE TABLE T ({columns}) CREATE TABLE S ({columns})")).unwrap();
    let s = db.session("X");
    for k in 0..ROWS {
        for table in ["T", "S"] {
            s.run(&format!("INSERT INTO {table} VALUES ({k}, {k}, 'w{k}', {k}E-1)")).unwrap();
        }
    }
    s.commit().unwrap();
    s.run("CREATE INDEX T_KV ON T (K, V) USING PROBING").unwrap();
    s
}

/// Runs the maintenance due of `db`, each zone settled as far as it can be.
fn settle(db: &Database) {
    for _ in 0..4 {
        db.run_due().unwrap();
    }
}

/// The rows of K = 1 that `s` reads of T, its index told by `flags` to
/// lie meanwhile: "1,1001", made of the entry, or none, the row read and
/// the entry refused.
fn row_one(s: &Session, flags: &Flags) -> Vec<String> {
    flags.lie.store(true, Ordering::SeqCst);
    let found = rows(s, "SELECT K, V FROM T WHERE K = 1").unwrap();
    flags.lie.store(false, Ordering::SeqCst);
    found
}

/// Checks that `sql`, of table T, gives `s` the rows the same query of S
/// gives it, its plan of T an index-only scan.
fn same_as_read(s: &Session, sql: &str) {
    let plan = s.explain(sql, &Params::new()).unwrap();
    assert!(plan.contains("IndexOnlyScan X.T using T_KV"), "{plan}");
    assert_eq!(sorted(s, sql), sorted(s, &sql.replace(" T ", " S ")), "{sql}");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §7.6 GR1]; [DESIGN] RULES.md
/// D34: a scan of an index holding every column read is an index-only
/// scan: a row of a zone settled visible is made of its entry, of one not
/// yet settled read; ranges, prefixes and an order read backward answer
/// as a scan of the table does. A column of a varying string or of an
/// approximate number is not restored from a key, so a query reading one
/// reads every row.
#[test]
fn rows_of_zones_settled_are_made_of_their_entries() {
    let flags = Arc::new(Flags::default());
    let db = database(&flags);
    let s = filled(&db);
    assert!(row_one(&s, &flags).is_empty(), "read, before maintenance settles its zone: the entry's lie refused");
    settle(&db);
    assert_eq!(row_one(&s, &flags), ["1,1001"], "made of its entry");
    for sql in [
        "SELECT K, V FROM T WHERE K BETWEEN 1000 AND 1100",
        "SELECT V FROM T WHERE K = 2050 AND V = 2050",
        "SELECT K FROM T WHERE K < 30 OR K > 2080",
        "SELECT K, V FROM T WHERE K > 2000 ORDER BY K DESC",
    ] {
        same_as_read(&s, sql);
    }
    let ordered = "SELECT K, V FROM T WHERE K > 2090 ORDER BY K DESC";
    assert_eq!(rows(&s, ordered).unwrap(), rows(&s, &ordered.replace(" T ", " S ")).unwrap(), "in order");
    flags.lie.store(true, Ordering::SeqCst);
    for column in ["W", "X"] {
        let sql = format!("SELECT K, {column} FROM T WHERE K = 1");
        let plan = s.explain(&sql, &Params::new()).unwrap();
        assert!(plan.contains("IndexScan X.T using T_KV") && !plan.contains("IndexOnly"), "{plan}");
        assert!(rows(&s, &sql).unwrap().is_empty(), "{column}: read, the lie refused");
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28]; [DESIGN] RULES.md
/// D29, D34: a zone written is read from before its write is made, by
/// every reader, until maintenance settles it again once no reader may
/// see an older version of its rows: a transaction sees its own update
/// and delete uncommitted, others the rows before them, a snapshot open
/// across the commit the rows before them still, after maintenance too.
#[test]
fn rows_written_are_read_until_settled_again() {
    let flags = Arc::new(Flags::default());
    let db = database(&flags);
    let s = filled(&db);
    settle(&db);
    let (reader, other) = (db.session("X"), db.session("X"));
    let range = "SELECT K, V FROM T WHERE K BETWEEN 0 AND 20";
    let before = sorted(&reader, range);
    for change in ["UPDATE T SET V = 100 WHERE K = 2", "DELETE FROM T WHERE K = 3", "INSERT INTO T VALUES (5000, 5000, 'w', 0)"] {
        s.run(change).unwrap();
        s.run(&change.replace(" T ", " S ")).unwrap();
    }
    let (theirs, mine) = (sorted(&other, range), sorted(&s, range));
    assert_eq!(theirs, before, "uncommitted: others see the rows before");
    assert!(mine.contains(&"2,100".to_owned()) && !mine.contains(&"3,3".to_owned()), "{mine:?}");
    assert!(row_one(&s, &flags).is_empty() && row_one(&other, &flags).is_empty(), "the zone read from the write on");
    s.commit().unwrap();
    other.commit().unwrap();
    settle(&db);
    assert_eq!(sorted(&reader, range), before, "a snapshot open across the commit");
    assert!(row_one(&reader, &flags).is_empty(), "not settled while a reader may see an older version");
    reader.commit().unwrap();
    settle(&db);
    assert_eq!(row_one(&s, &flags), ["1,1001"], "settled again");
    for sql in [range, "SELECT K, V FROM T WHERE K > 4000"] {
        same_as_read(&s, sql);
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28]; [DESIGN] RULES.md
/// D34: a zone of a row inserted while a reader of an older snapshot is
/// open is left to settle once none is: no version of it is collected to
/// look at it again.
#[test]
fn a_zone_inserted_into_settles_once_no_older_reader_is_open() {
    let flags = Arc::new(Flags::default());
    let db = database(&flags);
    let s = filled(&db);
    s.run("DELETE FROM T WHERE K = 1").unwrap();
    s.commit().unwrap();
    settle(&db);
    let reader = db.session("X");
    assert!(row_one(&reader, &flags).is_empty(), "K = 1 deleted");
    s.run("INSERT INTO T VALUES (1, 1, 'w1', 0)").unwrap();
    s.commit().unwrap();
    settle(&db);
    assert!(row_one(&s, &flags).is_empty(), "a commit past the horizon: read");
    reader.commit().unwrap();
    settle(&db);
    assert_eq!(row_one(&s, &flags), ["1,1001"], "settled once no older reader is open");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28]; [DESIGN] RULES.md
/// D34: the index entries of a version collected that its indexes fail
/// to take back, of two indexes here, are owed: the row's zone is read
/// from, so a stale entry is refused, until a later collection takes the
/// last back, and the zone settles visible again. Regression tests
/// (R-TEST-7): collection left such an entry for readers to check, and an
/// index-only scan gave its key for the row's; each entry taken back,
/// not each version, was counted, so the zone never settled again.
#[test]
fn entries_not_taken_back_keep_their_zone_read() {
    let flags = Arc::new(Flags::default());
    let db = database(&flags);
    let s = filled(&db);
    s.run("CREATE INDEX T_VK ON T (V, K) USING PROBING").unwrap();
    settle(&db);
    flags.fail.store(true, Ordering::SeqCst);
    s.run("UPDATE T SET V = 99 WHERE K = 5").unwrap();
    s.commit().unwrap();
    settle(&db);
    let five = "SELECT K, V FROM T WHERE K = 5";
    assert_eq!(rows(&s, five).unwrap(), ["5,99"], "the stale entry of 5,5 refused");
    flags.fail.store(false, Ordering::SeqCst);
    s.run("UPDATE T SET V = 7 WHERE K = 1500").unwrap();
    s.commit().unwrap();
    settle(&db);
    assert_eq!(row_one(&s, &flags), ["1,1001"], "taken back by the next collection, the zone settled");
    assert_eq!(rows(&s, five).unwrap(), ["5,99"]);
}

/// The database in `dir`, of the method `Probing` told by `flags`.
fn open(dir: &Path, flags: &Arc<Flags>) -> Database {
    let db = database(flags);
    db.open_files(&os(), dir, &OpenOptions::default()).unwrap();
    db
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28]; [DESIGN] RULES.md
/// D25, D34: a database opened reads every zone's rows until maintenance
/// settles them, whether it stopped without closing or closed, or closed
/// owing index entries, which opening it again recovers.
#[test]
fn zones_are_read_after_opening_until_settled() {
    let scratch = Scratch::new("index-only");
    let dir = scratch.path("db");
    let flags = Arc::new(Flags::default());
    let db = open(&dir, &flags);
    let s = filled(&db);
    s.run("UPDATE T SET V = V + 1 WHERE K BETWEEN 10 AND 20").unwrap();
    s.run("UPDATE S SET V = V + 1 WHERE K BETWEEN 10 AND 20").unwrap();
    s.commit().unwrap();
    drop((s, db));
    for round in 0..3 {
        flags.fail.store(false, Ordering::SeqCst);
        let db = open(&dir, &flags);
        let s = db.session("X");
        assert!(row_one(&s, &flags).is_empty(), "round {round}: read until settled");
        settle(&db);
        assert_eq!(row_one(&s, &flags), ["1,1001"], "round {round}: settled");
        same_as_read(&s, "SELECT K, V FROM T WHERE K < 40");
        if round == 2 {
            continue;
        }
        if round == 1 {
            flags.fail.store(true, Ordering::SeqCst);
            s.run("UPDATE T SET V = V - 1 WHERE K = 10").unwrap();
            s.run("UPDATE S SET V = V - 1 WHERE K = 10").unwrap();
            s.commit().unwrap();
        }
        drop(s);
        db.close().unwrap();
    }
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28]; [DESIGN] RULES.md
/// D29, D34: a writer, maintenance and a reader at once: each snapshot's
/// index-only scan gives the rows a scan of the table gives it.
#[test]
fn concurrent_writes_and_maintenance_keep_index_only_scans_right() {
    let flags = Arc::new(Flags::default());
    let db = Arc::new(database(&flags));
    drop(filled(&db));
    settle(&db);
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let (db, done) = (Arc::clone(&db), Arc::clone(&done));
        std::thread::spawn(move || {
            let s = db.session("X");
            for n in 0..300 {
                let k = n * 7 % 120;
                let changed = s.run(&format!("UPDATE T SET V = V + 1 WHERE K = {k}")).and_then(|_| s.commit());
                if changed.is_err() {
                    s.rollback().unwrap();
                }
            }
            done.store(true, Ordering::SeqCst);
        })
    };
    let maintainer = {
        let (db, done) = (Arc::clone(&db), Arc::clone(&done));
        std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                db.run_due().unwrap();
            }
        })
    };
    let s = db.session("X");
    let indexed = "SELECT K, V FROM T WHERE K < 120";
    assert!(s.explain(indexed, &Params::new()).unwrap().contains("IndexOnlyScan"));
    while !done.load(Ordering::SeqCst) {
        let (by_index, by_scan) = (sorted(&s, indexed), sorted(&s, "SELECT K, V FROM T WHERE K + 0 < 120"));
        assert_eq!(by_index, by_scan, "one snapshot");
        s.commit().unwrap();
    }
    writer.join().unwrap();
    maintainer.join().unwrap();
}
