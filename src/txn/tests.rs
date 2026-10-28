//! Unit tests of the transaction coordinator: commit reports a participant
//! it cannot find (RULES.md R-ERR-1, R-TEST-2 robustness); snapshots and
//! the horizon (RULES.md D29). Split from
//! txn.rs (RULES.md R-SIZE-2).

use super::*;
use crate::catalog::MemoryCatalog;

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
        data_type: crate::value::DataType::Exact { precision: 9, scale: 0 },
        not_null: false,
        default: None,
        domain: None,
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
        lock_builtin(&tx.state).joined.push(Joined { table: name.clone(), storage: Arc::clone(&storage) });
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
