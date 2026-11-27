//! Unit tests of the transaction coordinator: commit and rollback reach the
//! storage each participant joined; snapshots and the horizon, and reading
//! at a snapshot for each statement (RULES.md D29).

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
use crate::log::{Log, Lsn};
use crate::value::Spelled;

/// Verifies: [DESIGN] commit and rollback reach the storage each
/// participant joined, not the one its table's name finds: a row written
/// in a table whose name the definitions do not have is committed there,
/// or rolled back; ids go on, and no transaction is left open.
#[test]
fn participants_end_where_they_joined() {
    let st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let engine = crate::engine::rows::RowStore::in_memory(crate::engine::rows::TableOptions::default());
    let column = crate::catalog::ColumnDesc {
        name: "K".into(),
        data_type: crate::value::DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
        not_null: false,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    let desc = crate::catalog::TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns: vec![column],
        constraints: vec![],
        engine: "rowstore".into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let storage: Arc<dyn crate::storage::TableStorage> = Arc::from(crate::storage::StorageEngine::create_table(&engine, &desc).unwrap());
    let visible = |table: &Arc<dyn crate::storage::TableStorage>| {
        let reader = st.begin();
        table.begin(reader.id, reader.snapshot()).unwrap();
        let n = Arc::clone(table).scan(reader.id, &[0]).count();
        table.commit(reader.id, reader.snapshot());
        n
    };
    for (commits, rows) in [(true, 1), (false, 1)] {
        let tx = st.begin();
        storage.begin(tx.id, tx.snapshot()).unwrap();
        lock_builtin(&tx.state).joined.push(Joined { table: Arc::new(name.clone()), storage: Arc::clone(&storage) });
        lock_builtin(&tx.state).wrote = true;
        storage.insert(tx.id, vec![crate::value::Value::Null]).unwrap();
        if commits {
            st.commit(&tx).unwrap();
        } else {
            st.rollback(&tx).unwrap();
        }
        assert_eq!(visible(&storage), rows, "committed: {commits}");
        let next = st.begin();
        assert!(next.id > tx.id, "ids go on");
    }
    assert_eq!(st.tx.active(), 0);
}

/// Verifies: [DESIGN] RULES.md D29, stable snapshots: while a commit is
/// under way, transactions that begin read before its commit timestamp,
/// however late they begin, and once it has ended they read at it; the
/// horizon is the oldest open snapshot, counted once per transaction, ended
/// once whether by `release` or by being dropped, and with none open the
/// durable timestamp, a commit that syncs nothing durable as it ends.
#[test]
fn snapshots_never_count_a_commit_under_way() {
    let shared = Arc::new(Transactions::default());
    let committing = shared.commit_timestamp();
    let (a, b) = (
        shared.begin(None, (LockWait::NoWait, Reading::Serializable, Arc::default()), UndoSpace::default()),
        shared.begin(None, (LockWait::NoWait, Reading::Serializable, Arc::default()), UndoSpace::default()),
    );
    assert!(a.snapshot() < committing.at && b.snapshot() == a.snapshot(), "the commit has not ended");
    let at = committing.at;
    drop(committing);
    let c = shared.begin(None, (LockWait::NoWait, Reading::Serializable, Arc::default()), UndoSpace::default());
    assert_eq!(c.snapshot(), at, "it has ended");
    assert_eq!(shared.horizon(), a.snapshot());
    a.release();
    a.release();
    assert_eq!(shared.horizon(), b.snapshot(), "b still holds it");
    drop(b);
    assert_eq!(shared.horizon(), c.snapshot());
    drop((a, c));
    assert_eq!(shared.horizon(), at, "none open: the durable timestamp");
}

/// Verifies: [EXTENSION] [ISO/IEC 9075:1992 §4.28] (RULES.md D29): a
/// transaction reading at a snapshot for each statement takes the stable
/// timestamp as a statement begins, while its session has no cursor open,
/// and its old snapshot no longer holds the horizon back; one reading at
/// one snapshot, or told a cursor is open, keeps its own; its reads are
/// checked only while constraints are, a serializable one's always.
#[test]
fn a_statement_reads_at_a_snapshot_of_its_own() {
    let shared = Arc::new(Transactions::default());
    let begin = |reading| shared.begin(None, (LockWait::NoWait, reading, Arc::default()), UndoSpace::default());
    let (statement, snapshot, serializable) = (begin(Reading::Statement), begin(Reading::Snapshot), begin(Reading::Serializable));
    let first = statement.snapshot();
    drop(shared.commit_timestamp());
    for tx in [&statement, &snapshot, &serializable] {
        tx.begin_statement(|| true).unwrap();
    }
    assert!(statement.snapshot() > first, "a new snapshot");
    assert_eq!((snapshot.snapshot(), serializable.snapshot()), (first, first));
    drop(shared.commit_timestamp());
    statement.begin_statement(|| false).unwrap();
    assert_eq!(statement.snapshot(), first.saturating_add(1), "a cursor open");
    drop((snapshot, serializable));
    assert_eq!(shared.horizon(), statement.snapshot(), "the old snapshot released");
    assert!(!statement.read_mode().checked);
    statement.check_reads(true).unwrap();
    statement.check_reads(true).unwrap();
    assert!(statement.read_mode().checked, "while constraints are checked");
    statement.check_reads(false).unwrap();
    assert!(!statement.read_mode().checked);
    let strict = begin(Reading::Serializable);
    strict.check_reads(false).unwrap();
    assert!(strict.read_mode().checked, "always");
}

/// A log, in memory, that notes as each record is appended whether the
/// transaction it watches has its state locked, and drops the transaction
/// given it to end before the record is appended.
#[derive(Default)]
struct Watching {
    tx: OnceLock<std::sync::Weak<Txn>>,
    locked: Mutex<Vec<bool>>,
    ending: Mutex<Option<Txn>>,
    records: crate::log::MemoryLog,
}

impl Log for Watching {
    fn capabilities(&self) -> crate::log::LogCapabilities {
        self.records.capabilities()
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
        drop(lock_builtin(&self.ending).take());
        if let Some(tx) = self.tx.get().and_then(std::sync::Weak::upgrade) {
            let locked = matches!(tx.state.try_lock(), Err(std::sync::TryLockError::WouldBlock));
            lock_builtin(&self.locked).push(locked);
        }
        self.records.append(record)
    }
    fn sync(&self, through: Lsn) -> Result<()> {
        self.records.sync(through)
    }
    fn read_from(&self, from: Lsn) -> crate::log::Records<'_> {
        self.records.read_from(from)
    }
    fn truncate_before(&self, before: Lsn) -> Result<()> {
        self.records.truncate_before(before)
    }
    fn end(&self) -> Lsn {
        self.records.end()
    }

    fn bytes_between(&self, from: Lsn, to: Lsn) -> u64 {
        self.records.bytes_between(from, to)
    }
}

/// Verifies: [DESIGN] RULES.md R-ERR-4, D470: a transaction's state is not
/// locked while the log, a plug-in, appends its records: a change's held,
/// written by a checkpoint, nor the record of its statement undone after.
/// A statement whose records are all held is undone with none written.
/// Regression test (R-TEST-7): the state was held locked across the
/// append of a statement undone.
#[test]
fn the_log_appends_with_no_lock_held() {
    let shared = Arc::new(Transactions::default());
    let log = Arc::new(Watching::default());
    let watched: Arc<dyn Log> = log.clone();
    let tx = Arc::new(shared.begin(Some(&watched), (LockWait::NoWait, Reading::Serializable, Arc::default()), UndoSpace::default()));
    log.tx.set(Arc::downgrade(&tx)).unwrap();
    tx.log_change(crate::log::record::Change::Abort).unwrap();
    assert!(lock_builtin(&log.locked).is_empty(), "held, not written");
    shared.write_held().unwrap();
    tx.statement_undone();
    shared.write_held().unwrap();
    assert_eq!(*lock_builtin(&log.locked), [false, false]);
    tx.begin_statement(|| false).unwrap();
    tx.log_change(crate::log::record::Change::Abort).unwrap();
    tx.statement_undone();
    shared.write_held().unwrap();
    assert_eq!(lock_builtin(&log.locked).len(), 2, "held and undone: nothing written");
}

/// Verifies: [DESIGN] RULES.md D25, D44, D470: a transaction whose records
/// a checkpoint wrote holds the log from its first until it ends, dropped;
/// one holding them unwritten, or none, holds nothing, and its end leaves
/// the other's held; what
/// cancels an open transaction's statements is found by its id, whichever
/// shard holds it, as of the thread it began on, until it ends.
#[test]
fn a_transactions_first_record_and_stop_last_until_it_ends() {
    let shared = Arc::new(Transactions::default());
    let log: Arc<dyn Log> = Arc::new(crate::log::MemoryLog::default());
    let origin = Arc::new(Origin::new(&shared, Some(&log), UndoSpace::default()));
    let begin = || origin.begin((LockWait::NoWait, Reading::Serializable, Arc::default()));
    let (writer, reader) = (begin(), begin());
    writer.log_change(crate::log::record::Change::Abort).unwrap();
    assert!(shared.oldest_logged().is_none(), "held, not written");
    shared.write_held().unwrap();
    assert!(shared.oldest_logged().is_some(), "the writer's first");
    // Begun on threads of their own too, so their ids are on other
    // threads' stripes, and in other shards (RULES.md D844).
    let mut more: Vec<Txn> = (0_u32..8_u32).map(|_| begin()).collect();
    for _ in 0_u32..4_u32 {
        let on = Arc::clone(&origin);
        more.push(std::thread::spawn(move || on.begin((LockWait::NoWait, Reading::Serializable, Arc::default()))).join().unwrap());
    }
    for tx in [&writer, &reader].into_iter().chain(&more) {
        assert!(shared.stop_of(tx.id).is_some(), "open: found in its shard");
    }
    let (writer_id, reader_id) = (writer.id, reader.id);
    drop(reader);
    assert!(shared.oldest_logged().is_some(), "one that logged nothing leaves the writer's");
    assert!(shared.stop_of(reader_id).is_none(), "ended");
    drop(writer);
    assert_eq!(shared.oldest_logged(), None, "forgotten as the writer ended");
    assert!(shared.stop_of(writer_id).is_none());
    assert!(more.iter().all(|tx| shared.stop_of(tx.id).is_some()), "the others still open");
}

/// The record of row `row` of a table deleted.
fn deletion(row: u64) -> crate::log::record::Change {
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    crate::log::record::Change::Delete { table, generation: 0, row, before: Vec::new() }
}

/// The records `log` holds, in order: of each, its transaction, and
/// whether it is an abort record.
fn logged(log: &dyn Log) -> Vec<(TxId, bool)> {
    let types = crate::value::types::TypeRegistry::default();
    let read = |bytes: Vec<u8>| crate::log::record::Record::decode(&bytes, &types).unwrap();
    let records = log.read_from(Lsn(0)).map(|at| read(at.unwrap().1));
    records.map(|record| (record.tx, matches!(record.change, crate::log::record::Change::Abort))).collect()
}

/// Verifies: [X3.135-1986 §8.9]; [DESIGN] RULES.md D25, D470, D1250, a
/// transaction rolled back and kept after, as its session keeps it until
/// the rollback's call ends: the records it held are dropped as it rolls
/// back, so a checkpoint's write of what open transactions hold, made
/// before the transaction is dropped, writes none of them and notes no
/// first record the log would be kept from; one whose first record a
/// checkpoint wrote before has its abort record as its last, the records
/// it held since unwritten, and its first forgotten as it is dropped.
/// Regression test (R-TEST-7): the records stayed in its buffer until it
/// was dropped, a checkpoint between wrote them with no abort record
/// after, and recovery, reading them as of a transaction a crash
/// interrupted, put its rows back over what was committed there since.
#[test]
fn a_rollback_leaves_no_record_for_a_checkpoint_to_write() {
    let mut st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let log: Arc<dyn Log> = Arc::new(crate::log::MemoryLog::default());
    st.log = Some(Arc::clone(&log));
    let held = st.begin();
    held.log_change(deletion(1)).unwrap();
    st.rollback(&held).unwrap();
    st.tx.write_held().unwrap();
    assert!(logged(&*log).is_empty(), "held as it rolled back: no record of it is written after");
    assert_eq!(st.tx.oldest_logged(), None, "and none holds the log");
    let written = st.begin();
    written.log_change(deletion(2)).unwrap();
    st.tx.write_held().unwrap();
    written.log_change(deletion(3)).unwrap();
    st.rollback(&written).unwrap();
    st.tx.write_held().unwrap();
    assert_eq!(logged(&*log), [(written.id, false), (written.id, true)], "its abort record is its last");
    assert!(st.tx.oldest_logged().is_some(), "its first, until it is dropped");
    drop((held, written));
    assert_eq!(st.tx.oldest_logged(), None);
}

/// Verifies: [DESIGN] RULES.md D25, D470, D1250, a transaction dropped
/// while a checkpoint writes the records it holds, after the write took
/// them and before it notes their first: the record is written, and no
/// first is noted for a transaction that is over, so nothing holds the
/// log from it. Regression test (R-TEST-7): the drop found no first
/// noted, the write noted one after, and nothing forgot it, so every
/// later checkpoint's image started at that record, the log was kept
/// from it, and recovery read from it.
#[test]
fn a_transaction_dropped_as_its_records_are_written_holds_no_log() {
    let shared = Arc::new(Transactions::default());
    let log = Arc::new(Watching::default());
    let watched: Arc<dyn Log> = log.clone();
    let tx = shared.begin(Some(&watched), (LockWait::NoWait, Reading::Serializable, Arc::default()), UndoSpace::default());
    tx.log_change(deletion(1)).unwrap();
    *lock_builtin(&log.ending) = Some(tx);
    shared.write_held().unwrap();
    assert!(lock_builtin(&log.ending).is_none(), "dropped as its record was appended");
    assert_eq!(logged(&log.records).len(), 1, "taken before it was dropped: written");
    assert_eq!(shared.oldest_logged(), None, "over: no first is noted for it");
}

/// A table that records how the transactions that joined it ended, and
/// refuses their rollbacks if told to.
#[derive(Default)]
struct Ends {
    ends: Mutex<Vec<&'static str>>,
    refuses: bool,
}

impl crate::storage::TableStorage for Ends {
    fn insert(&self, _tx: TxId, _row: crate::storage::Row) -> Result<crate::storage::RowId> {
        Ok(0)
    }
    fn update(&self, _tx: TxId, _id: crate::storage::RowId, _row: crate::storage::Row) -> Result<()> {
        Ok(())
    }
    fn delete(&self, _tx: TxId, _id: crate::storage::RowId) -> Result<()> {
        Ok(())
    }
    fn restore(&self, _tx: TxId, _id: crate::storage::RowId, _row: crate::storage::Row) -> Result<()> {
        Ok(())
    }
    fn get(&self, _tx: TxId, _id: crate::storage::RowId) -> Result<Option<crate::storage::Row>> {
        Ok(None)
    }
    fn scan(self: Arc<Self>, _tx: TxId, _columns: &[usize]) -> crate::storage::Rows {
        Box::new(std::iter::empty())
    }
    fn row_count(&self) -> usize {
        0
    }
    fn commit(&self, _tx: TxId, _at: Timestamp) {
        lock_builtin(&self.ends).push("commit");
    }
    fn rollback(&self, _tx: TxId) -> Result<()> {
        lock_builtin(&self.ends).push("rollback");
        if self.refuses { Err(SqlError::new(crate::error::Code::Io, "refused")) } else { Ok(()) }
    }
}

/// A state logging to a memory log, and a transaction of it that joined
/// `table`, holds a table's lock and one record, written to the log if
/// `written`: the state, the log, the lock and the transaction.
fn joined(table: &Arc<Ends>, written: bool) -> (State, Arc<dyn Log>, Arc<TableLock>, Txn) {
    let mut st = State::with_builtins(Box::new(MemoryCatalog::default()));
    let log: Arc<dyn Log> = Arc::new(crate::log::MemoryLog::default());
    st.log = Some(Arc::clone(&log));
    let tx = st.begin();
    let name = Arc::new(QualifiedName { auth: "A".into(), ident: "T".into() });
    let lock = Arc::new(TableLock::default());
    let storage: Arc<dyn crate::storage::TableStorage> = Arc::<Ends>::clone(table);
    {
        let mut state = lock_builtin(&tx.state);
        state.joined.push(Joined { table: name, storage });
        state.held.push(Arc::clone(&lock));
    }
    tx.log_change(deletion(1)).unwrap();
    if written {
        st.tx.write_held().unwrap();
    }
    (st, log, lock, tx)
}

/// Verifies: [X3.135-1986 §8.9]; [DESIGN] RULES.md D25, D1259, a
/// transaction dropped still open, neither committed nor rolled back, as
/// a function that began one of its own leaves it on an early error: the
/// drop rolls it back, each engine it joined told, its abort record after
/// the record a checkpoint wrote of it, nothing left holding the log, the
/// database taking changes still and its table not damaged; one that only
/// read is ended in its engine too, with no record; one ended already,
/// rolled back, is not ended again by its drop. Regression test
/// (R-TEST-7): the drop released its locks and nothing more, so its
/// versions stayed in the engine with no end, the engine never told.
#[test]
fn a_transaction_dropped_open_is_rolled_back() {
    let table = Arc::new(Ends::default());
    let (st, log, lock, tx) = joined(&table, true);
    let id = tx.id;
    drop(tx);
    assert_eq!(*lock_builtin(&table.ends), ["rollback"]);
    assert_eq!(logged(&*log), [(id, false), (id, true)], "its abort record after its record");
    assert_eq!(st.tx.oldest_logged(), None);
    assert!(st.tx.taking_changes().is_ok() && lock.damage().is_none());
    let reader = st.begin();
    let name = Arc::new(QualifiedName { auth: "A".into(), ident: "T".into() });
    let storage: Arc<dyn crate::storage::TableStorage> = Arc::<Ends>::clone(&table);
    lock_builtin(&reader.state).joined.push(Joined { table: name, storage });
    drop(reader);
    assert_eq!(*lock_builtin(&table.ends), ["rollback", "rollback"], "one that only read");
    assert_eq!(logged(&*log).len(), 2, "which logs nothing");
    let (again, _, _, ended) = joined(&table, false);
    again.rollback(&ended).unwrap();
    drop(ended);
    assert_eq!(lock_builtin(&table.ends).len(), 3, "ended once");
    assert_eq!(again.tx.active(), 0);
}

/// Drops, as `dropped` drops it, a transaction that joined `table`, and
/// asserts that its database then takes no change, naming the
/// transaction dropped (SQLCODE -915), and that the table it holds is
/// marked damaged.
fn stops(table: &Arc<Ends>, dropped: impl FnOnce(Txn)) {
    let (st, _, lock, tx) = joined(table, false);
    dropped(tx);
    let refused = st.tx.taking_changes().unwrap_err();
    assert!(refused.code == crate::error::Code::Io && refused.msg.contains("dropped before its end"), "{refused}");
    assert!(lock.damage().is_some());
}

/// Verifies: [DESIGN] RULES.md D19, D1258, D1259, a transaction dropped
/// still open that cannot be rolled back: its engine refusing the
/// rollback, its state's lock poisoned by a panic, dropped while a panic
/// unwinds, or its commit under way, its participants taken and its undo
/// entries not: the database stops taking changes (SQLCODE -915, naming
/// the transaction dropped), the table it holds is marked damaged, and
/// where the rollback could not be trusted the engine is asked nothing.
#[test]
fn a_transaction_that_cannot_be_ended_stops_the_database() {
    stops(&Arc::new(Ends { refuses: true, ..Ends::default() }), drop);
    let table = Arc::new(Ends::default());
    stops(&table, |tx| {
        let poisoning = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = tx.state.lock();
            panic!("a panic holding the transaction's state");
        }));
        assert!(poisoning.is_err() && tx.state.is_poisoned());
    });
    stops(&table, |tx| {
        let unwinding = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _dropped_unwinding = tx;
            panic!("a panic with the transaction open");
        }));
        assert!(unwinding.is_err());
    });
    stops(&table, |tx| {
        let mut state = lock_builtin(&tx.state);
        let taken = std::mem::take(&mut state.joined);
        let first = taken.first().unwrap();
        let (name, storage) = (QualifiedName::clone(&first.table), Arc::clone(&first.storage));
        state.undo.push(UndoEntry::Inserted { table: name, storage, id: 1 }).unwrap();
    });
    assert!(lock_builtin(&table.ends).is_empty(), "no engine asked where the rollback could not be trusted");
}

/// Verifies: [DESIGN] RULES.md D29: maintenance is due from a table's note,
/// looked at without a lock, until the tables noted are taken, each once.
#[test]
fn maintenance_is_due_from_a_note_until_taken() {
    let shared = Transactions::default();
    assert!(!shared.maintenance_due(), "none noted");
    let table = QualifiedName { auth: "A".into(), ident: "T".into() };
    shared.note_maintenance(&table);
    shared.note_maintenance(&table);
    assert!(shared.maintenance_due());
    assert_eq!(shared.take_maintenance().len(), 1);
    assert!(!shared.maintenance_due(), "taken");
}
