//! Unit tests of the transaction coordinator: commit and rollback reach the
//! storage each participant joined; snapshots and the horizon, and reading
//! at a snapshot for each statement (RULES.md D29).

use super::*;
use crate::catalog::MemoryCatalog;
use crate::db::State;
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
/// stable timestamp.
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
    assert_eq!(shared.horizon(), at, "none open: the stable timestamp");
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
/// transaction it watches has its state locked.
#[derive(Default)]
struct Watching {
    tx: std::sync::OnceLock<std::sync::Weak<Txn>>,
    locked: Mutex<Vec<bool>>,
    records: crate::log::MemoryLog,
}

impl Log for Watching {
    fn capabilities(&self) -> crate::log::LogCapabilities {
        self.records.capabilities()
    }
    fn append(&self, record: &[u8]) -> Result<Lsn> {
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
}

/// Verifies: [DESIGN] RULES.md R-ERR-4: a transaction's state is not
/// locked while the log, a plug-in, appends its records: a change's, nor
/// the record of its statement undone.
/// Regression test (R-TEST-7): the state was held locked across the
/// append of a statement undone.
#[test]
fn the_log_appends_with_no_lock_held() {
    let shared = Arc::new(Transactions::default());
    let log = Arc::new(Watching::default());
    let watched: Arc<dyn Log> = log.clone();
    let tx = Arc::new(shared.begin(Some(watched), (LockWait::NoWait, Reading::Serializable, Arc::default()), UndoSpace::default()));
    log.tx.set(Arc::downgrade(&tx)).unwrap();
    tx.log_change(crate::log::record::Change::Abort).unwrap();
    tx.statement_undone();
    assert_eq!(*lock_builtin(&log.locked), [false, false]);
}

/// Verifies: [DESIGN] RULES.md D25, D44: a transaction that logged a
/// record holds the log from its first until it ends, dropped; one that
/// logged none holds nothing, and its end leaves the other's held; what
/// cancels an open transaction's statements is found by its id, whichever
/// shard holds it, until it ends.
#[test]
fn a_transactions_first_record_and_stop_last_until_it_ends() {
    let shared = Arc::new(Transactions::default());
    let log: Arc<dyn Log> = Arc::new(crate::log::MemoryLog::default());
    let begin = || shared.begin(Some(Arc::clone(&log)), (LockWait::NoWait, Reading::Serializable, Arc::default()), UndoSpace::default());
    let (writer, reader) = (begin(), begin());
    writer.log_change(crate::log::record::Change::Abort).unwrap();
    assert!(shared.oldest_logged().is_some(), "the writer's first");
    let more: Vec<Txn> = (0_u32..40_u32).map(|_| begin()).collect();
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
