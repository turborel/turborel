//! Commit through the log (RULES.md D25): a transaction's
//! row changes are logged as made, and it commits when its commit record is
//! durable; a rollback, a failed statement and changes of definitions are
//! logged too; what a failing log does to each; and the engines'
//! persistence: whose changes are logged, and what `Strict` refuses.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use turborel::value::types::TypeRegistry;

use turborel::catalog::TableDesc;
use turborel::db::Database;
use turborel::error::{Code, Result, SqlError};
use turborel::log::record::{Change, Record};
use turborel::log::{Log, LogCapabilities, Lsn, MemoryLog, Records};
use turborel::storage::{Row, RowId, Rows, StorageEngine, TableStorage};
use turborel::txn::{Persistence, Policy, TxCapabilities, TxId};
use turborel::value::Value;

use crate::common::{MemoryTable, count, crc32c, os, setup};

/// A memory log that records which positions were synced, and fails its
/// appends or syncs while told to.
#[derive(Default)]
struct Watched {
    inner: MemoryLog,
    synced: Mutex<Vec<Lsn>>,
    fail_appends: AtomicBool,
    fail_syncs: AtomicBool,
    /// Whether it says it is durable: by default, not.
    durable: bool,
    /// Appends that still succeed before the rest fail, if set.
    appends_left: Mutex<Option<usize>>,
}

impl Log for Watched {
    fn capabilities(&self) -> LogCapabilities {
        LogCapabilities { durable: self.durable }
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

/// The database of `setup`, logging to a fresh watched log.
fn logged() -> (Database, Arc<Watched>) {
    let db = setup();
    let log = Arc::new(Watched::default());
    db.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    (db, log)
}

/// The records in `log`, with their positions.
fn records(log: &Watched) -> Vec<(Lsn, Record)> {
    log.read_from(Lsn(0))
        .map(|r| r.map(|(lsn, bytes)| (lsn, Record::decode(&bytes, &TypeRegistry::default()).unwrap())))
        .collect::<Result<_>>()
        .unwrap()
}

/// The kinds of record in `log`, in order.
fn kinds(log: &Watched) -> Vec<String> {
    records(log).iter().map(|(_, r)| format!("{:?}", r.change).split([' ', '{']).next().unwrap().to_owned()).collect()
}

/// Verifies: [X3.135-1986 §8.2 GR3]; [DESIGN] RULES.md D25: a transaction's
/// inserts, updates and deletes are logged as they are made, with their
/// row ids and images, then its commit record with its commit timestamp,
/// all under its transaction id; COMMIT returns once the commit record is
/// synced.
#[test]
fn a_commit_is_its_record_made_durable() {
    let (db, log) = logged();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID, NAME) VALUES (10, 'TEN')").unwrap();
    s.run("UPDATE T SET NAME = 'X' WHERE ID = 10").unwrap();
    s.run("DELETE FROM T WHERE ID = 1").unwrap();
    let tx = s.transaction().unwrap();
    s.commit().unwrap();
    assert_eq!(kinds(&log), ["Insert", "Update", "Delete", "Commit"]);
    let all = records(&log);
    assert!(all.iter().all(|(_, r)| r.tx == tx));
    let (commit_lsn, commit) = all.last().unwrap();
    assert!(matches!(commit.change, Change::Commit { at } if at > 0), "with its commit timestamp");
    assert_eq!(*log.synced.lock().unwrap(), vec![*commit_lsn], "synced through its commit record");
    if let Change::Update { before, after, .. } = &all[1].1.change {
        assert_eq!((before[1].to_string(), after[1].to_string()), ("TEN   ".to_owned(), "X     ".to_owned()));
    }
}

/// Verifies: [DESIGN] RULES.md D25, D36: a session whose LOG.COMMIT_SYNC
/// is OFF commits once its commit record is written, unsynced, its rows
/// seen by others at once; the next commit of a session of ON syncs
/// through its own record, and so the first's; the database's value is
/// every session's that sets none; a value not ON or OFF is refused.
#[test]
fn a_commit_need_not_wait_for_the_log() {
    let (db, log) = logged();
    let (quick, careful) = (db.session("APP"), db.session("APP"));
    let off = Value::Char("OFF".to_owned());
    quick.set_setting("LOG.COMMIT_SYNC", &off).unwrap();
    quick.run("INSERT INTO T (ID, NAME) VALUES (10, 'TEN')").unwrap();
    quick.commit().unwrap();
    assert_eq!(kinds(&log), ["Insert", "Commit"]);
    assert!(log.synced.lock().unwrap().is_empty(), "not synced");
    assert_eq!(count(&careful, "SELECT ID FROM T WHERE ID = 10"), 1, "seen at once");
    careful.run("INSERT INTO T (ID, NAME) VALUES (11, 'ELEVEN')").unwrap();
    careful.commit().unwrap();
    let last = records(&log).last().map(|(lsn, _)| *lsn).unwrap();
    assert_eq!(*log.synced.lock().unwrap(), vec![last], "through both commit records");
    db.set_setting("LOG.COMMIT_SYNC", &off).unwrap();
    let synced = log.synced.lock().unwrap().len();
    careful.run("INSERT INTO T (ID, NAME) VALUES (12, 'TWELVE')").unwrap();
    careful.commit().unwrap();
    assert_eq!(log.synced.lock().unwrap().len(), synced, "the database's OFF");
    let maybe = quick.set_setting("LOG.COMMIT_SYNC", &Value::Char("MAYBE".to_owned()));
    assert_eq!(maybe.map_err(|e| e.sqlcode()), Err(-910_i32));
}

/// Verifies: [X3.135-1986 §8.9]; [DESIGN] RULES.md D25: a rolled-back
/// transaction logs its changes, then an abort record, and syncs nothing; a
/// transaction that changed nothing logs nothing, and its commit syncs
/// nothing.
#[test]
fn rollbacks_and_reads_are_logged_as_needed() {
    let (db, log) = logged();
    let s = db.session("APP");
    assert_eq!(count(&s, "SELECT ID FROM T"), 3);
    s.commit().unwrap();
    assert!(records(&log).is_empty(), "a read-only transaction logs nothing");
    s.run("INSERT INTO T (ID) VALUES (11)").unwrap();
    s.rollback().unwrap();
    assert_eq!(kinds(&log), ["Insert", "Abort"]);
    assert!(log.synced.lock().unwrap().is_empty());
}

/// Verifies: [X3.135-1986 §7.3 GR3b i]; [DESIGN] RULES.md D25: a statement
/// that fails after changing rows is undone, and logged as undone from its
/// first change, so recovery does not replay what it cancelled; the
/// transaction goes on and commits.
#[test]
fn a_failed_statement_is_logged_undone() {
    let (db, log) = logged();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    // T.ID is UNIQUE: the row is inserted, found a duplicate, and undone.
    assert_eq!(s.run("INSERT INTO T (ID) VALUES (10)").err().map(|e| e.code), Some(Code::Constraint));
    s.commit().unwrap();
    let all = records(&log);
    assert_eq!(kinds(&log), ["Insert", "Insert", "StatementUndone", "Commit"]);
    assert!(matches!(all[2].1.change, Change::StatementUndone { from } if from == all[1].0), "undone from its own first change");
}

/// Verifies: [DESIGN] RULES.md D25, a failing log: a change that cannot be
/// logged fails its statement (SQLCODE -915), which is undone; a statement
/// none of whose changes reached the log leaves nothing to mark undone, and
/// the transaction commits; one whose first changes were logged, and whose
/// undoing then cannot be, leaves the transaction able only to roll back:
/// COMMIT rolls it back (SQLCODE -913); a commit record that cannot be
/// written rolls the transaction back too.
#[test]
fn changes_that_cannot_be_logged_do_not_commit() {
    let (db, log) = logged();
    let s = db.session("APP");
    log.fail_appends.store(true, Ordering::SeqCst);
    assert_eq!(s.run("INSERT INTO T (ID) VALUES (9)").err().map(|e| e.code), Some(Code::Io));
    log.fail_appends.store(false, Ordering::SeqCst);
    s.commit().unwrap();
    assert_eq!(count(&s, "SELECT ID FROM T"), 3, "the insert was undone");
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    // The UPDATE's first row is logged, its second is not, nor its undoing.
    *log.appends_left.lock().unwrap() = Some(1);
    assert_eq!(s.run("UPDATE T SET NAME = 'Z'").err().map(|e| e.code), Some(Code::Io));
    *log.appends_left.lock().unwrap() = None;
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed), "the failed statement's undoing was not logged");
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T"), 3, "rolled back: no row 10");
    s.run("INSERT INTO T (ID) VALUES (12)").unwrap();
    log.fail_appends.store(true, Ordering::SeqCst);
    assert_eq!(s.commit().err().map(|e| e.code), Some(Code::CommitFailed), "no commit record");
    log.fail_appends.store(false, Ordering::SeqCst);
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T WHERE ID = 12"), 0);
}

/// Verifies: [DESIGN] RULES.md D25, a failing log: when the commit record is
/// written but cannot be synced, COMMIT reports the transaction in doubt
/// (SQLCODE -922, SQLSTATE 08007, transaction resolution unknown [ISO/IEC
/// 9075:1992 §4.29, §22.1]): no session sees its changes, its own neither,
/// and it stays open, held until the database is dropped. From then on the
/// database takes no change, though its log syncs again, as a later sync
/// may succeed without what the failed one lost: a row change, the commit
/// of a transaction that wrote, a definition, each SQLCODE -915 naming the
/// failure; reads, rollbacks and commits of transactions that wrote
/// nothing go on. Regression test (R-TEST-7): the transaction was stamped
/// committed, and other sessions saw what recovery could lose.
#[test]
fn a_failed_sync_stops_the_database_taking_changes() {
    let (db, log) = logged();
    let s = db.session("APP");
    assert!(db.log_failure().is_none());
    s.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    log.fail_syncs.store(true, Ordering::SeqCst);
    let error = s.commit().unwrap_err();
    assert_eq!((error.code, error.sqlcode(), error.sqlstate().as_str()), (Code::InDoubt, -922, "08007"));
    assert!(error.msg.contains("opened again"), "{error}");
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T WHERE ID = 10"), 0, "another session");
    assert_eq!(count(&s, "SELECT ID FROM T WHERE ID = 10"), 0, "its own session, in a new transaction");
    assert_eq!(db.active_transactions().unwrap(), 2, "held in doubt, beside the reading one");
    assert_eq!(kinds(&log).last().map(String::as_str), Some("Commit"));
    log.fail_syncs.store(false, Ordering::SeqCst);
    let cause = db.log_failure().expect("the failure kept");
    assert_eq!(cause.code, Code::Io);
    let refused = s.run("INSERT INTO T (ID) VALUES (11)").unwrap_err();
    assert_eq!(refused.code, Code::Io);
    assert!(refused.msg.contains("opened again"), "{refused}");
    s.rollback().unwrap();
    assert_eq!(count(&s, "SELECT ID FROM T"), 3, "reads go on");
    s.commit().unwrap();
    assert_eq!(db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (K INTEGER)").err().map(|e| e.code), Some(Code::Io));
    assert_eq!(kinds(&log).last().map(String::as_str), Some("Commit"), "nothing logged since, no abort among it");
}

/// The newest segment of the log of the database in `dir`.
fn last_segment(dir: &std::path::Path) -> std::path::PathBuf {
    let mut segments: Vec<std::path::PathBuf> = std::fs::read_dir(dir.join("log")).unwrap().map(|e| e.unwrap().path()).collect();
    segments.sort();
    segments.pop().unwrap()
}

/// Verifies: [DESIGN] RULES.md D25, a commit in doubt, over the operating
/// system's files: a commit whose log sync fails is reported in doubt
/// (SQLCODE -922, SQLSTATE 08007), and told as a `failed` event of level
/// error; another session does not see its rows,
/// and no writer is taken (-915); it stays held, so the database is not
/// closed cleanly (-915); opened again, it is recovered from what the log
/// holds: committed if its commit record reached the disk, its rows there;
/// rolled back if not (the record cut off, as a write lost with the failed
/// sync), its rows gone. Regression test (R-TEST-7): a commit whose sync
/// failed was stamped committed, its rows seen by other sessions though
/// recovery could lose them.
#[test]
fn a_commit_whose_sync_fails_is_in_doubt_until_recovered() {
    use turborel::db::{OpenOptions, Opened};

    use crate::faults::{FaultyFs, Op, Rule};

    for reached in [true, false] {
        let scratch = crate::common::Scratch::new(&format!("durability-in-doubt-{reached}"));
        let dir = scratch.path("db");
        let (fs, faults) = FaultyFs::with_faults();
        let db = Database::new();
        let kept = crate::common::Kept::default();
        db.set_diagnostics(Box::new(kept.clone()));
        db.open_files(&fs, &dir, &OpenOptions::default()).unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION A CREATE TABLE T (K INTEGER)").unwrap();
        let (s, other) = (db.session("A"), db.session("A"));
        s.run("INSERT INTO T VALUES (1)").unwrap();
        s.commit().unwrap();
        s.run("INSERT INTO T VALUES (2)").unwrap();
        s.run("INSERT INTO T VALUES (3)").unwrap();
        let segment = last_segment(&dir);
        let before_commit = std::fs::metadata(&segment).unwrap().len();
        faults.inject(Rule::fail(Op::Sync).on("/log/"));
        let error = s.commit().unwrap_err();
        faults.clear();
        assert_eq!((error.code, error.sqlstate().as_str()), (Code::InDoubt, "08007"), "{error}");
        let told = kept.taken();
        assert!(
            told.iter().any(|(level, e)| *level == turborel::diagnostics::Level::Error && e.contains("Failed") && e.contains("InDoubt")),
            "told as an error: {told:?}"
        );
        assert_eq!(count(&other, "SELECT K FROM T"), 1, "not seen by another session");
        assert_eq!(other.run("INSERT INTO T VALUES (4)").err().map(|e| e.code), Some(Code::Io), "no writer");
        other.rollback().unwrap();
        assert_eq!(db.close().err().map(|e| e.code), Some(Code::Io), "no clean close");
        drop((s, other, db));
        assert!(std::fs::metadata(&segment).unwrap().len() > before_commit, "the commit record written");
        if !reached {
            let file = std::fs::OpenOptions::new().write(true).open(&segment).unwrap();
            file.set_len(before_commit).unwrap();
        }
        let again = Database::new();
        let opened = again.open_files(&os(), &dir, &OpenOptions::default()).unwrap();
        assert!(matches!(opened, Opened::Recovered(_)), "{opened:?}");
        let expected = if reached { 3 } else { 1 };
        assert_eq!(count(&again.session("A"), "SELECT K FROM T"), expected, "reached the disk: {reached}");
        again.session("A").run("INSERT INTO T VALUES (5)").unwrap();
    }
}

/// Verifies: [DESIGN] RULES.md D25: a transaction that wrote before the
/// log's sync failed cannot commit after it: it is rolled back (SQLCODE
/// -913), naming the failure.
#[test]
fn a_writer_open_when_the_log_fails_rolls_back() {
    let (db, log) = logged();
    let (writer, other) = (db.session("APP"), db.session("APP"));
    writer.run("INSERT INTO T (ID) VALUES (20)").unwrap();
    other.run("INSERT INTO T (ID) VALUES (21)").unwrap();
    log.fail_syncs.store(true, Ordering::SeqCst);
    assert_eq!(other.commit().err().map(|e| e.code), Some(Code::InDoubt));
    log.fail_syncs.store(false, Ordering::SeqCst);
    let refused = writer.commit().unwrap_err();
    assert_eq!(refused.code, Code::CommitFailed);
    assert!(refused.msg.contains("sync of the log failed"), "{refused}");
    assert_eq!(count(&db.session("APP"), "SELECT ID FROM T WHERE ID = 20"), 0);
}

/// Verifies: [X3.135-1986 §6.1]; [DESIGN] RULES.md D25: a `<schema>` and an
/// index are logged, and synced, before they are published; if the log
/// cannot take them, they are refused and the definitions stay as they were.
#[test]
fn definitions_are_logged_before_they_are_published() {
    let (db, log) = logged();
    db.schema("CREATE SCHEMA AUTHORIZATION B CREATE TABLE S (K INTEGER NOT NULL)").unwrap();
    db.create_index("S_K", "B.S", &["K"], "btree").unwrap();
    assert_eq!(kinds(&log), ["Schema", "CreateIndex"]);
    assert_eq!(log.synced.lock().unwrap().len(), 2);
    assert!(
        matches!(&records(&log)[0].1.change, Change::Schema { sql, engines, .. } if sql.contains("CREATE TABLE S") && engines == &[("S".to_owned(), "rowstore".to_owned())])
    );
    log.fail_appends.store(true, Ordering::SeqCst);
    assert_eq!(db.schema("CREATE SCHEMA AUTHORIZATION C CREATE TABLE U (K INTEGER)").err().map(|e| e.code), Some(Code::Io));
    log.fail_appends.store(false, Ordering::SeqCst);
    let c = db.session("C");
    assert_eq!(c.run("INSERT INTO U VALUES (1)").err().map(|e| e.code), Some(Code::SyntaxRule), "C.U was not published");
}

/// Verifies: [DESIGN] RULES.md D25: by default a database logs nothing; a
/// log set later is used by transactions begun from then on, while one
/// already open keeps the log it began with.
#[test]
fn the_log_is_chosen_when_a_transaction_begins() {
    let db = setup();
    let early = db.session("APP");
    early.run("INSERT INTO T (ID) VALUES (10)").unwrap();
    let log = Arc::new(Watched::default());
    db.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    early.commit().unwrap();
    assert!(records(&log).is_empty(), "begun before the log was set");
    let later = db.session("APP");
    later.run("INSERT INTO T (ID) VALUES (11)").unwrap();
    later.commit().unwrap();
    assert_eq!(kinds(&log), ["Insert", "Commit"]);
}

/// Verifies: [DESIGN] RULES.md D25: with a file log, a committed
/// transaction's records are in the log's files once COMMIT returns: a log
/// opened anew from them holds its changes and its commit.
#[test]
fn a_file_log_holds_what_was_committed() {
    use turborel::log::{FileLog, FileLogOptions};

    let scratch = crate::common::Scratch::new("durability-file");
    let dir = scratch.path("log");
    let hash: Arc<dyn turborel::hash::HashFunction> = crc32c();
    let log = FileLog::create(os(), &dir, Arc::clone(&hash), FileLogOptions::default()).unwrap();
    let db = setup();
    db.set_log(Arc::new(log)).unwrap();
    let s = db.session("APP");
    s.run("INSERT INTO T (ID, NAME) VALUES (10, 'TEN')").unwrap();
    s.commit().unwrap();
    let (reopened, _) = FileLog::open(os(), &dir, &[hash], &[], FileLogOptions::default()).unwrap();
    let kinds: Vec<String> = reopened
        .read_from(Lsn(0))
        .map(|r| {
            format!("{:?}", Record::decode(&r.unwrap().1, &TypeRegistry::default()).unwrap().change)
                .split([' ', '{'])
                .next()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["Insert", "Commit"]);
}

/// An engine that declares the persistence it is made with, its rows a
/// `MemoryTable`'s.
struct Declared(Persistence);

struct DeclaredTable {
    rows: MemoryTable,
    persistence: Persistence,
}

impl StorageEngine for Declared {
    fn name(&self) -> &'static str {
        match self.0 {
            Persistence::Logged => "logged",
            Persistence::Forced => "forced",
            Persistence::External => "external",
            Persistence::None => "scratch",
        }
    }
    fn create_table(&self, _: &TableDesc) -> Result<Box<dyn TableStorage>> {
        Ok(Box::new(DeclaredTable { rows: MemoryTable::new(), persistence: self.0 }))
    }
}

impl TableStorage for DeclaredTable {
    fn insert(&self, tx: TxId, row: Row) -> Result<RowId> {
        self.rows.insert(tx, row)
    }
    fn update(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.update(tx, id, row)
    }
    fn delete(&self, tx: TxId, id: RowId) -> Result<()> {
        self.rows.delete(tx, id)
    }
    fn restore(&self, tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.rows.restore(tx, id, row)
    }
    fn get(&self, tx: TxId, id: RowId) -> Result<Option<Row>> {
        self.rows.get(tx, id)
    }
    fn scan(self: Arc<Self>, _: TxId, _: &[usize]) -> Rows {
        self.rows.live_rows()
    }
    fn row_count(&self) -> usize {
        self.rows.row_count()
    }
    fn capabilities(&self) -> TxCapabilities {
        TxCapabilities { persistence: self.persistence, ..TxCapabilities::default() }
    }
}

/// A database with schema P holding table L, F, E and N in engines that
/// declare `Logged`, `Forced`, `External` and `None`, under `policy`, with
/// a log whose durability is `durable`; and the log.
fn declared(policy: Policy, durable: bool) -> (Database, Arc<Watched>) {
    let db = Database::new();
    for (persistence, table) in
        [(Persistence::Logged, "L"), (Persistence::Forced, "F"), (Persistence::External, "E"), (Persistence::None, "N")]
    {
        let engine = Declared(persistence);
        let name = engine.name();
        db.register_engine(Box::new(engine)).unwrap();
        db.place_table(&format!("P.{table}"), name).unwrap();
    }
    db.schema("CREATE SCHEMA AUTHORIZATION P CREATE TABLE L (K INTEGER) CREATE TABLE F (K INTEGER) CREATE TABLE E (K INTEGER) CREATE TABLE N (K INTEGER)")
        .unwrap();
    db.set_transaction_policy(policy).unwrap();
    let log = Arc::new(Watched { durable, ..Watched::default() });
    db.set_log(Arc::clone(&log) as Arc<dyn Log>).unwrap();
    (db, log)
}

/// The code of inserting into `table` in a transaction of its own, which
/// is then rolled back.
fn insert_alone(db: &Database, table: &str) -> Option<Code> {
    let s = db.session("P");
    let code = s.run(&format!("INSERT INTO {table} VALUES (1)")).err().map(|e| e.code);
    s.rollback().unwrap();
    code
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D25, persistence under
/// `Strict`: a `Forced` engine is written only with a durable log, a `None`
/// engine only without one, and a `Logged` one always; `Permissive` allows
/// them all.
///
/// MC/DC of the two refusals (persistence, log durable): `Forced` with and
/// without a durable log, against `Logged` without one; `None` with and
/// without one, against `Logged` with one.
#[test]
fn strict_refuses_persistence_the_log_cannot_back() {
    let (durable, _) = declared(Policy::Strict, true);
    let (volatile, _) = declared(Policy::Strict, false);
    assert_eq!((insert_alone(&durable, "F"), insert_alone(&volatile, "F")), (None, Some(Code::Policy)));
    assert_eq!((insert_alone(&durable, "N"), insert_alone(&volatile, "N")), (Some(Code::Policy), None));
    assert_eq!((insert_alone(&durable, "L"), insert_alone(&volatile, "L")), (None, None));
    let (permissive, _) = declared(Policy::Permissive, false);
    assert_eq!(insert_alone(&permissive, "F"), None);
}

/// Verifies: [X3.135-1986 §4.16]; [DESIGN] RULES.md D25, persistence under
/// `Strict`: a transaction that writes an `External` engine's table writes
/// no other, in either order; a refused write leaves the transaction as it
/// was, so it can go on writing the kind it began with.
#[test]
fn an_external_engine_is_written_alone() {
    let (db, _) = declared(Policy::Strict, true);
    let s = db.session("P");
    s.run("INSERT INTO E VALUES (1)").unwrap();
    assert_eq!(s.run("INSERT INTO L VALUES (1)").err().map(|e| e.code), Some(Code::Policy));
    s.run("INSERT INTO E VALUES (2)").unwrap();
    s.commit().unwrap();
    s.run("INSERT INTO L VALUES (1)").unwrap();
    assert_eq!(s.run("INSERT INTO E VALUES (3)").err().map(|e| e.code), Some(Code::Policy));
    s.run("INSERT INTO F VALUES (1)").unwrap();
    s.commit().unwrap();
    let (permissive, _) = declared(Policy::Permissive, true);
    let p = permissive.session("P");
    p.run("INSERT INTO E VALUES (1)").unwrap();
    p.run("INSERT INTO L VALUES (1)").unwrap();
}

/// Verifies: [DESIGN] RULES.md D25: the changes of tables the database
/// recovers (`Logged`, `Forced`) are logged; those of `External` and
/// `None` engines are not, as they are never redone or undone from the log.
#[test]
fn only_recovered_engines_are_logged() {
    let (db, log) = declared(Policy::Permissive, true);
    let s = db.session("P");
    for table in ["L", "F", "E", "N"] {
        s.run(&format!("INSERT INTO {table} VALUES (1)")).unwrap();
    }
    s.commit().unwrap();
    let tables: Vec<String> = records(&log)
        .into_iter()
        .filter_map(|(_, r)| match r.change {
            Change::Insert { table, .. } => Some(table.ident),
            _ => None,
        })
        .collect();
    assert_eq!(tables, ["L", "F"]);
}

/// Verifies: [DESIGN] RULES.md D25: `TableStorage::apply`, by which
/// recovery redoes and undoes, puts or removes a row by id whatever it
/// holds, and applying a change twice is as applying it once; an engine
/// keeping nothing is durable through position 0.
#[test]
fn applying_a_change_twice_is_applying_it_once() {
    let table = MemoryTable::new();
    let row = || vec![Value::Char("v".into())];
    for _ in 0..2 {
        table.apply(5, Some(row())).unwrap();
    }
    assert_eq!((table.row_count(), table.get(0, 5).unwrap().map(|r| r[0].to_string())), (1, Some("v".to_owned())));
    table.apply(5, Some(vec![Value::Char("w".into())])).unwrap();
    assert_eq!(table.get(0, 5).unwrap().map(|r| r[0].to_string()), Some("w".to_owned()));
    for _ in 0..2 {
        table.apply(5, None).unwrap();
    }
    assert_eq!((table.row_count(), table.get(0, 5).unwrap().is_none()), (0, true));
    assert_eq!(table.durable_through(), Lsn(0));
}
