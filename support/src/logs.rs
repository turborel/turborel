//! The suites every log passes (RULES.md D25, D57, R-SAFE-3, R-TEST-10):
//! the `Log` contract, and what a database writes to its log and gets back
//! from it, through each kind of log a test names (`LogKind`). The core's
//! integration tests run them for its built-in logs, including this file
//! (`#[path]`) with `database.rs`; replication's for its replicated log,
//! through this crate. Each suite is a function, called by a test of each
//! binary, which states what it verifies.
#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::panic_in_result_fn,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::too_many_lines,
    clippy::default_numeric_fallback,
    clippy::wildcard_enum_match_arm,
    clippy::cognitive_complexity,
    clippy::shadow_unrelated,
    reason = "test code: a panic is a test failure, its fixtures literal, a database rebound once dropped (RULES.md R-TEST-4)"
)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use turborel::catalog::QualifiedName;
use turborel::db::{Database, LogStorage, OpenOptions, Opened, RecoveryReport};
use turborel::disk::io::Durability;
use turborel::error::{Code, Result, SqlError};
use turborel::log::record::{Change, Record};
use turborel::log::{FileLog, FileLogOptions, Log, LogCapabilities, Lsn, MemoryLog, Records};
use turborel::value::types::TypeRegistry;
use turborel::value::{Decimal, Value};

use crate::database::{LOCKING, LockingEngine, after_crash, count, crc32c, os, rows, setup, small_pool, sorted};

/// A kind of log the suites run on: how a test makes one in a directory,
/// opens it again as after a restart, and keeps a database's log with it.
pub trait LogKind {
    /// A new log in `dir`.
    fn create(&self, dir: &Path) -> Arc<dyn Log>;
    /// The log in `dir`, opened again.
    fn open(&self, dir: &Path) -> Arc<dyn Log>;
    /// What keeps the log of a database opened from its files, if not the
    /// built-in (`Database::set_log_storage`).
    fn storage(&self) -> Option<Box<dyn LogStorage>>;
}

/// The built-in file log, of the options it is made with, its checksum
/// CRC-32C; a database's own, of the directory's `log/`.
pub struct BuiltIn(pub FileLogOptions);

impl LogKind for BuiltIn {
    fn create(&self, dir: &Path) -> Arc<dyn Log> {
        Arc::new(FileLog::create(os(), dir, crc32c(), self.0.clone()).unwrap())
    }
    fn open(&self, dir: &Path) -> Arc<dyn Log> {
        Arc::new(FileLog::open(os(), dir, &[crc32c()], &[], self.0.clone()).unwrap().0)
    }
    fn storage(&self) -> Option<Box<dyn LogStorage>> {
        None
    }
}

/// Record `n`: different lengths, some longer than a segment's share.
pub fn record(n: usize) -> Vec<u8> {
    format!("record {n}: {}", "x".repeat(n * 37 % 700)).into_bytes()
}

/// The records of `log` from `from`, with their positions.
pub fn all(log: &dyn Log, from: Lsn) -> Vec<(Lsn, Vec<u8>)> {
    log.read_from(from).collect::<Result<Vec<_>>>().unwrap()
}

/// The contract every log keeps: positions increase with appends that do
/// not overlap; `read_from` yields exactly what was appended, in order,
/// from any position given, or one record at a position; `sync` accepts
/// the positions given and refuses others; `truncate_before` never removes a record at or after its
/// position; records appended by threads at once are each read once, each
/// thread's in the order it appended them.
pub fn contract(log: &dyn Log) {
    let start = log.end();
    assert!(all(log, Lsn(0)).is_empty() && all(log, start).is_empty(), "a new log is empty");
    let positions: Vec<Lsn> = (0..40).map(|n| log.append(&record(n)).unwrap()).collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "positions increase");
    assert!(*positions.last().unwrap() < log.end());
    let read = all(log, Lsn(0));
    assert_eq!(read, positions.iter().copied().zip((0..40).map(record)).collect::<Vec<_>>());
    assert_eq!(all(log, positions[25]), read[25..].to_vec(), "from a position");
    assert!(all(log, log.end()).is_empty(), "from the end");
    for at in [0, 17, 39] {
        assert_eq!(log.read_at(positions[at]).unwrap(), Some(record(at)), "the record at a position");
    }
    assert_eq!(log.read_at(log.end()).unwrap(), None, "none at the end");
    log.sync(positions[39]).unwrap();
    assert_eq!(log.sync(Lsn(u64::MAX)).err().map(|e| e.code), Some(Code::Api), "no such position");
    log.truncate_before(positions[30]).unwrap();
    let kept = all(log, Lsn(0));
    assert!(kept.ends_with(&read[30..]), "nothing at or after the position removed");
    assert_eq!(log.read_at(positions[30]).unwrap(), Some(record(30)), "still read at the position");
    let threads: Vec<Vec<Lsn>> = thread::scope(|s| {
        let appending: Vec<_> = (0..4)
            .map(|t| s.spawn(move || (0..25).map(|n| log.append(format!("thread {t} record {n}").as_bytes()).unwrap()).collect()))
            .collect();
        appending.into_iter().map(|a| a.join().unwrap()).collect()
    });
    assert!(threads.iter().all(|mine: &Vec<Lsn>| mine.windows(2).all(|w| w[0] < w[1])), "each thread's appends in order");
    let after = all(log, positions[39]);
    assert_eq!(after.len(), 1 + 4 * 25, "every record appended by the threads, once");
    assert!(threads.iter().flatten().all(|p| after.iter().any(|(q, _)| q == p)));
    for t in 0..4 {
        let prefix = format!("thread {t} record ");
        let read: Vec<String> =
            after.iter().filter_map(|(_, r)| String::from_utf8_lossy(r).strip_prefix(&prefix).map(str::to_owned)).collect();
        assert_eq!(read, (0..25).map(|n| n.to_string()).collect::<Vec<_>>(), "thread {t}'s records read in its order");
    }
}

/// A transaction's records (rows inserted, updated and deleted, then its
/// commit) appended to a log of `kind` in `dir`, synced at the commit, and
/// read back after it is opened again, decode to what was written, in
/// order.
pub fn records_go_through(kind: &dyn LogKind, dir: &Path) {
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    let number = |n: i128| Value::Exact(Decimal::new(n, 0));
    let records = [
        Record {
            tx: 5,
            change: Change::Insert { table: table.clone(), generation: 0, row: 1, after: vec![number(1), Value::Char("one".into())] },
        },
        Record {
            tx: 5,
            change: Change::Update { table: table.clone(), generation: 0, row: 1, before: vec![number(1)], after: vec![number(2)] },
        },
        Record { tx: 5, change: Change::Delete { table, generation: 0, row: 1, before: vec![number(2)] } },
        Record { tx: 5, change: Change::Commit { at: 9 } },
    ];
    let log = kind.create(dir);
    let mut commit = Lsn(0);
    for r in &records {
        commit = log.append(&r.encode()).unwrap();
    }
    log.sync(commit).unwrap();
    drop(log);
    let log = kind.open(dir);
    let read: Vec<Vec<u8>> =
        all(&*log, Lsn(0)).into_iter().map(|(_, bytes)| Record::decode(&bytes, &TypeRegistry::default()).unwrap().encode()).collect();
    assert_eq!(read, records.iter().map(Record::encode).collect::<Vec<_>>());
}

/// A log over another, `inner`, that records which positions were synced,
/// and the durabilities it is told (`Log::set_durability`), passed on to
/// `inner`; and fails its appends or syncs while told to.
pub struct Watched {
    /// The log it watches.
    pub inner: Arc<dyn Log>,
    /// The positions synced, in order.
    pub synced: Mutex<Vec<Lsn>>,
    /// Whether appends fail.
    pub fail_appends: AtomicBool,
    /// Whether syncs fail.
    pub fail_syncs: AtomicBool,
    /// Whether it says it is durable.
    pub durable: bool,
    /// Whether it says it is durable as `inner` does, not as `durable` does.
    pub follows: bool,
    /// The durabilities it was told, in order.
    pub told: Mutex<Vec<Durability>>,
    /// Appends that still succeed before the rest fail, if set.
    pub appends_left: Mutex<Option<usize>>,
}

impl Watched {
    /// It over `inner`, saying it is not durable.
    pub fn over(inner: Arc<dyn Log>) -> Watched {
        Watched {
            inner,
            synced: Mutex::default(),
            fail_appends: AtomicBool::default(),
            fail_syncs: AtomicBool::default(),
            durable: false,
            follows: false,
            told: Mutex::default(),
            appends_left: Mutex::default(),
        }
    }
}

/// It over a memory log.
impl Default for Watched {
    fn default() -> Watched {
        Watched::over(Arc::new(MemoryLog::new()))
    }
}

impl Log for Watched {
    fn capabilities(&self) -> LogCapabilities {
        if self.follows { self.inner.capabilities() } else { LogCapabilities { durable: self.durable } }
    }
    fn set_durability(&self, durability: Durability) {
        self.told.lock().unwrap().push(durability);
        self.inner.set_durability(durability);
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        let mut left = self.appends_left.lock().unwrap();
        if self.fail_appends.load(Ordering::SeqCst) || *left == Some(0) {
            return Err(SqlError::new(Code::Io, "injected: the log cannot append"));
        }
        if let Some(n) = left.as_mut() {
            *n -= 1;
        }
        drop(left);
        self.inner.append(record)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        if self.fail_syncs.load(Ordering::SeqCst) {
            return Err(SqlError::new(Code::Io, "injected: the log cannot sync"));
        }
        self.synced.lock().unwrap().push(through);
        self.inner.sync(through)
    }
    fn read_from(&self, from: Lsn) -> Records<'_> {
        self.inner.read_from(from)
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        self.inner.truncate_before(before)
    }
    fn end(&self) -> Lsn {
        self.inner.end()
    }
}

/// The database of `setup`, logging to `log`, watched.
pub fn logged(log: Arc<dyn Log>) -> (Database, Arc<Watched>) {
    let db = setup();
    let watched = Arc::new(Watched::over(log));
    db.set_log(Arc::clone(&watched) as Arc<dyn Log>).unwrap();
    (db, watched)
}

/// The records in `log`, with their positions.
pub fn records(log: &dyn Log) -> Vec<(Lsn, Record)> {
    log.read_from(Lsn(0))
        .map(|r| r.map(|(lsn, bytes)| (lsn, Record::decode(&bytes, &TypeRegistry::default()).unwrap())))
        .collect::<Result<_>>()
        .unwrap()
}

/// The kinds of record in `log`, in order.
pub fn kinds(log: &dyn Log) -> Vec<String> {
    records(log).iter().map(|(_, r)| format!("{:?}", r.change).split([' ', '{']).next().unwrap().to_owned()).collect()
}

/// A transaction's inserts, updates and deletes, logged to `log` as they
/// are made, with their row ids and images, then its commit record with its
/// commit timestamp, all under its transaction id; COMMIT returns once the
/// commit record is synced.
pub fn a_commit_is_its_record_made_durable(log: Arc<dyn Log>) {
    let (db, log) = logged(log);
    let s = db.session("APP");
    s.run("INSERT INTO T (ID, NAME) VALUES (10, 'TEN')").unwrap();
    s.run("UPDATE T SET NAME = 'X' WHERE ID = 10").unwrap();
    s.run("DELETE FROM T WHERE ID = 1").unwrap();
    let tx = s.transaction().unwrap();
    s.commit().unwrap();
    assert_eq!(kinds(&*log), ["Insert", "Update", "Delete", "Commit"]);
    let all = records(&*log);
    assert!(all.iter().all(|(_, r)| r.tx == tx));
    let (commit_lsn, commit) = all.last().unwrap();
    assert!(matches!(commit.change, Change::Commit { at } if at > 0), "with its commit timestamp");
    assert_eq!(*log.synced.lock().unwrap(), vec![*commit_lsn], "synced through its commit record");
    if let Change::Update { before, after, .. } = &all[1].1.change {
        assert_eq!((before[1].to_string(), after[1].to_string()), ("TEN   ".to_owned(), "X     ".to_owned()));
    }
}

/// A session whose LOG.COMMIT_SYNC is OFF commits once its commit record is
/// written to `log`, unsynced, its rows seen by others at once; the next
/// commit of a session of ON syncs through its own record, and so the
/// first's; the database's value is every session's that sets none; a
/// value not ON or OFF is refused. The log's own sync job
/// (LOG.SYNC_AFTER_MS) is left to the program here.
pub fn a_commit_need_not_wait_for_the_log(log: Arc<dyn Log>) {
    let db = setup();
    db.set_job_policy(turborel::threads::JobPolicy::Manual).unwrap();
    let log = Arc::new(Watched::over(log));
    db.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    let (quick, careful) = (db.session("APP"), db.session("APP"));
    let off = Value::Char("OFF".to_owned());
    quick.set_setting("LOG.COMMIT_SYNC", &off).unwrap();
    quick.run("INSERT INTO T (ID, NAME) VALUES (10, 'TEN')").unwrap();
    quick.commit().unwrap();
    assert_eq!(kinds(&*log), ["Insert", "Commit"]);
    assert!(log.synced.lock().unwrap().is_empty(), "not synced");
    assert_eq!(count(&careful, "SELECT ID FROM T WHERE ID = 10"), 1, "seen at once");
    careful.run("INSERT INTO T (ID, NAME) VALUES (11, 'ELEVEN')").unwrap();
    careful.commit().unwrap();
    let last = records(&*log).last().map(|(lsn, _)| *lsn).unwrap();
    assert_eq!(*log.synced.lock().unwrap(), vec![last], "through both commit records");
    db.set_setting("LOG.COMMIT_SYNC", &off).unwrap();
    let synced = log.synced.lock().unwrap().len();
    careful.run("INSERT INTO T (ID, NAME) VALUES (12, 'TWELVE')").unwrap();
    careful.commit().unwrap();
    assert_eq!(log.synced.lock().unwrap().len(), synced, "the database's OFF");
    let maybe = quick.set_setting("LOG.COMMIT_SYNC", &Value::Char("MAYBE".to_owned()));
    assert_eq!(maybe.map_err(|e| e.sqlcode()), Err(-910_i32));
}

/// A rolled-back transaction logs its changes to `log`, then an abort
/// record, and syncs nothing; a transaction that changed nothing logs
/// nothing, and its commit syncs nothing.
pub fn rollbacks_and_reads_are_logged_as_needed(log: Arc<dyn Log>) {
    let (db, log) = logged(log);
    let s = db.session("APP");
    assert_eq!(count(&s, "SELECT ID FROM T"), 3);
    s.commit().unwrap();
    assert!(records(&*log).is_empty(), "a read-only transaction logs nothing");
    s.run("INSERT INTO T (ID) VALUES (11)").unwrap();
    s.rollback().unwrap();
    assert_eq!(kinds(&*log), ["Insert", "Abort"]);
    assert!(log.synced.lock().unwrap().is_empty());
}

/// A statement that fails after changing rows is undone, and logged to
/// `log` as undone from its first change, so recovery does not replay what
/// it cancelled; the transaction goes on and commits.
pub fn a_failed_statement_is_logged_undone(log: Arc<dyn Log>) {
    let (db, log) = logged(log);
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    // T.ID is UNIQUE: the row is inserted, found a duplicate, and undone.
    assert_eq!(s.run("INSERT INTO T (ID) VALUES (10)").err().map(|e| e.code), Some(Code::Constraint));
    s.commit().unwrap();
    let all = records(&*log);
    assert_eq!(kinds(&*log), ["Insert", "Insert", "StatementUndone", "Commit"]);
    assert!(matches!(all[2].1.change, Change::StatementUndone { from } if from == all[1].0), "undone from its own first change");
}

/// A `<schema>` and an index are logged to `log`, and synced, before they
/// are published; if the log cannot take them, they are refused and the
/// definitions stay as they were.
pub fn definitions_are_logged_before_they_are_published(log: Arc<dyn Log>) {
    let (db, log) = logged(log);
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (K INTEGER NOT NULL)").unwrap();
    db.create_index("S_K", "B.S", &["K"], "btree").unwrap();
    assert_eq!(kinds(&*log), ["Schema", "CreateIndex"]);
    assert_eq!(log.synced.lock().unwrap().len(), 2);
    assert!(
        matches!(&records(&*log)[0].1.change, Change::Schema { sql, engines, .. } if sql.contains("CREATE TABLE S") && engines == &[("S".to_owned(), "rowstore".to_owned())])
    );
    log.fail_appends.store(true, Ordering::SeqCst);
    assert_eq!(db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE U (K INTEGER)").err().map(|e| e.code), Some(Code::Io));
    log.fail_appends.store(false, Ordering::SeqCst);
    let c = db.session("C");
    assert_eq!(c.run("INSERT INTO U VALUES (1)").err().map(|e| e.code), Some(Code::SyntaxRule), "C.U was not published");
}

/// A committed transaction's records are in a log of `kind` in `dir` once
/// COMMIT returns: the log opened anew holds its changes and its commit.
pub fn a_log_holds_what_was_committed(kind: &dyn LogKind, dir: &Path) {
    let db = setup();
    db.set_log(kind.create(dir)).unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID, NAME) VALUES (10, 'TEN')").unwrap();
    s.commit().unwrap();
    assert_eq!(kinds(&*kind.open(dir)), ["Insert", "Commit"]);
}

/// A new database recovered from `log`.
pub fn recovered(log: Arc<dyn Log>) -> (Database, RecoveryReport) {
    let db = Database::new();
    let report = db.recover(log).unwrap();
    (db, report)
}

/// After a crash, a database rebuilt from its log, of `kind` in `dir`,
/// holds the committed transactions' rows and only those: not a
/// rolled-back one's, not a failed statement's, not those of a transaction
/// still open at the crash; its `<schema>`, index and UNIQUE constraint are
/// back, the index rebuilt from the rows; new transactions take ids past
/// the log's; and recovering again, from the log grown since, gives the
/// same rows and the new ones.
pub fn a_crashed_database_recovers_what_was_committed(kind: &dyn LogKind, dir: &Path) {
    let db = Database::new();
    db.set_log(kind.create(dir)).unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER NOT NULL UNIQUE, V CHAR(4))").unwrap();
    db.create_index("T_V", "A.T", &["V"], "hash").unwrap();
    let s = db.session("A");
    for k in 1..=4 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'v{k}')")).unwrap();
    }
    s.commit().unwrap();
    s.run("UPDATE T SET V = 'new' WHERE K = 2").unwrap();
    s.run("DELETE FROM T WHERE K = 3").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO T VALUES (9, 'gone')").unwrap();
    s.rollback().unwrap();
    s.run("INSERT INTO T VALUES (5, 'v5')").unwrap();
    assert!(s.run("INSERT INTO T VALUES (5, 'dup')").is_err(), "UNIQUE: undone");
    s.commit().unwrap();
    let expected = sorted(&db.session("A"), "SELECT K, V FROM T");
    // A transaction still open at the crash.
    let open = db.session("A");
    open.run("INSERT INTO T VALUES (7, 'open')").unwrap();
    open.run("UPDATE T SET V = 'open' WHERE K = 1").unwrap();
    let (back, report) = recovered(kind.open(dir));
    assert_eq!(sorted(&back.session("A"), "SELECT K, V FROM T"), expected);
    assert_eq!(expected, ["1,v1  ", "2,new ", "4,v4  ", "5,v5  "]);
    assert_eq!((report.definitions, report.committed, report.not_committed), (2, 3, 2), "the rollback and the open transaction did not");
    assert_eq!(rows(&back.session("A"), "SELECT K FROM T WHERE V = 'new'"), Ok(vec!["2".into()]), "the index works");
    let b = back.session("A");
    assert_eq!(b.run("INSERT INTO T VALUES (1, 'x')").err().map(|e| e.code), Some(Code::Constraint), "UNIQUE is back");
    b.run("INSERT INTO T VALUES (8, 'v8')").unwrap();
    assert!(b.transaction().unwrap() > open.transaction().unwrap(), "past the log's transaction ids");
    b.commit().unwrap();
    drop((b, back));
    let (again, _) = recovered(kind.open(dir));
    assert_eq!(sorted(&again.session("A"), "SELECT K FROM T"), ["1", "2", "4", "5", "8"]);
}

/// The database in `dir`, its log kept as `kind` keeps it, opened with a
/// small buffer pool, so pages are written back while it runs;
/// `LockingEngine` registered first.
pub fn open_kept(kind: &dyn LogKind, dir: &Path) -> (Database, Opened) {
    let db = Database::new();
    db.register_engine(Box::new(LockingEngine)).unwrap();
    if let Some(storage) = kind.storage() {
        db.set_log_storage(storage).unwrap();
    }
    small_pool(&db, 32);
    let opened = db.open_files(&after_crash(), dir, &OpenOptions::default()).unwrap();
    (db, opened)
}

/// Schema APP: T in the row store, with a UNIQUE key, a B-link index and a
/// hash index; L in `LockingEngine`, whose tables keep nothing.
pub fn define_kept(db: &Database) {
    db.place_table("APP.L", LOCKING).unwrap();
    db.schema(
        "CREATE SCHEMA AUTHORIZATION APP
           CREATE TABLE T (K INTEGER NOT NULL UNIQUE, NAME CHAR(20), AMT DECIMAL(9,2))
           CREATE TABLE L (K INTEGER NOT NULL)",
    )
    .unwrap();
    db.create_index("T_NAME", "APP.T", &["NAME"], "btree").unwrap();
    db.create_index("T_AMT", "APP.T", &["AMT"], "hash").unwrap();
}

/// 3,000 rows of T and 10 of L, committed; some updated and deleted; a
/// transaction rolled back, and a later one changing the same row.
pub fn fill_kept(db: &Database) {
    let s = db.session("APP");
    for k in 0..3_000 {
        s.run(&format!("INSERT INTO T VALUES ({k}, 'N{k}', {})", k % 50)).unwrap();
    }
    for k in 0..10 {
        s.run(&format!("INSERT INTO L VALUES ({k})")).unwrap();
    }
    s.commit().unwrap();
    s.run("UPDATE T SET NAME = 'changed' WHERE K < 100").unwrap();
    s.run("DELETE FROM T WHERE K >= 2900").unwrap();
    s.commit().unwrap();
    s.run("UPDATE T SET AMT = 999 WHERE K = 5").unwrap();
    s.rollback().unwrap();
    s.run("UPDATE T SET AMT = 7 WHERE K = 5").unwrap();
    s.commit().unwrap();
}

/// What the tests of a database kept compare before and after reopening.
pub fn answers(db: &Database) -> Vec<std::result::Result<Vec<String>, Code>> {
    let s = db.session("APP");
    [
        "SELECT COUNT(*), SUM(AMT) FROM T",
        "SELECT K FROM T WHERE NAME = 'N777'",
        "SELECT COUNT(*) FROM T WHERE NAME = 'changed'",
        "SELECT COUNT(*) FROM T WHERE AMT = 5",
        "SELECT AMT FROM T WHERE K = 5",
        "SELECT COUNT(*) FROM L",
    ]
    .iter()
    .map(|q| rows(&s, q))
    .collect()
}

/// A database in `dir`, its log of `kind`, closed cleanly and opened again
/// is reopened in place: its rows, its B-link index (used by the planner),
/// its hash index, its UNIQUE constraint, and a table of an engine that
/// keeps nothing, replayed from the log; new rows go on with new ids, and
/// it closes and reopens again; a closed database refuses every call.
pub fn a_closed_database_reopens_in_place(kind: &dyn LogKind, dir: &Path) {
    let (db, opened) = open_kept(kind, dir);
    assert_eq!(opened, Opened::Created);
    define_kept(&db);
    fill_kept(&db);
    let before = answers(&db);
    assert_eq!(before[0], Ok(vec!["2900,71052.00".into()]));
    assert_eq!(before[4], Ok(vec!["7.00".into()]));
    db.close().unwrap();
    assert_eq!(db.session("APP").run("COMMIT WORK").err().map(|e| e.code), Some(Code::Api), "closed");
    assert_eq!(db.close().err().map(|e| e.code), Some(Code::Api));
    drop(db);

    let (db, opened) = open_kept(kind, dir);
    assert_eq!(opened, Opened::Reopened);
    assert_eq!(answers(&db), before);
    let s = db.session("APP");
    assert!(
        s.physical_plan("SELECT K FROM T WHERE NAME = 'N777'", &turborel::params::Params::new()).unwrap().contains("T_NAME"),
        "the reopened index is used"
    );
    assert_eq!(s.run("INSERT INTO T VALUES (7, 'dup', 1)").err().map(|e| e.code), Some(Code::Constraint), "UNIQUE");
    s.run("INSERT INTO T VALUES (5000, 'new', 1)").unwrap();
    s.run("INSERT INTO L VALUES (99)").unwrap();
    s.commit().unwrap();
    drop(s);
    db.close().unwrap();
    drop(db);

    let (db, opened) = open_kept(kind, dir);
    assert_eq!(opened, Opened::Reopened);
    let s = db.session("APP");
    assert_eq!(rows(&s, "SELECT K FROM T WHERE NAME = 'new'"), Ok(vec!["5000".into()]));
    assert_eq!(rows(&s, "SELECT COUNT(*) FROM L"), Ok(vec!["11".into()]));
}

/// A database in `dir`, its log of `kind`, that stops without closing is
/// recovered from its log: what was committed, a row a rolled-back
/// transaction changed and a later one committed holding the later value,
/// and nothing of a transaction open when it stopped; then it closes and
/// reopens in place.
pub fn a_database_not_closed_is_recovered(kind: &dyn LogKind, dir: &Path) {
    let (crashed, _) = open_kept(kind, dir);
    define_kept(&crashed);
    fill_kept(&crashed);
    let s = crashed.session("APP");
    s.run("UPDATE T SET AMT = 1 WHERE K = 3").unwrap();
    s.rollback().unwrap();
    s.run("UPDATE T SET AMT = 2 WHERE K = 3").unwrap();
    s.commit().unwrap();
    let expected = answers(&crashed);
    let open_tx = crashed.session("APP");
    open_tx.run("INSERT INTO T VALUES (8000, 'open', 1)").unwrap();
    open_tx.run("DELETE FROM L").unwrap();

    let (db, opened) = open_kept(kind, dir);
    assert!(matches!(opened, Opened::Recovered(report) if report.committed >= 3), "{opened:?}");
    assert_eq!(answers(&db), expected);
    assert_eq!(rows(&db.session("APP"), "SELECT AMT FROM T WHERE K = 3"), Ok(vec!["2.00".into()]), "the later commit's value");
    assert_eq!(rows(&db.session("APP"), "SELECT K FROM T WHERE NAME = 'open'"), Ok(vec![]));
    db.close().unwrap();
    drop(db);
    let (db, opened) = open_kept(kind, dir);
    assert_eq!(opened, Opened::Reopened);
    assert_eq!(answers(&db), expected);
    drop((db, open_tx, s));
}
