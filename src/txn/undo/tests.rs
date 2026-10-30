//! Unit tests of the undo log (`txn/undo.rs`): entries spilled and read
//! back, a refused area, a run that cannot be read, a record not of an
//! entry (RULES.md R-TEST-1, R-TEST-2 robustness).

use std::sync::Mutex;

use super::*;
use crate::error::Code;
use crate::index::Probe;
use crate::spill::Spill;
use crate::spill::testing::{Held, areas};
use crate::storage::Rows;
use crate::sync::lock_builtin;
use crate::value::Value;

/// What a recorder was asked to do, in order.
type Calls = Arc<Mutex<Vec<String>>>;

/// A table, or an index, that records each call made of it.
struct Recorder(Calls);

impl Recorder {
    fn note(&self, call: String) {
        lock_builtin(&self.0).push(call);
    }
}

impl TableStorage for Recorder {
    fn insert(&self, _tx: TxId, _row: Row) -> Result<RowId> {
        Ok(0)
    }
    fn update(&self, _tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.note(format!("update {id} {row:?}"));
        Ok(())
    }
    fn delete(&self, _tx: TxId, id: RowId) -> Result<()> {
        self.note(format!("delete {id}"));
        Ok(())
    }
    fn restore(&self, _tx: TxId, id: RowId, row: Row) -> Result<()> {
        self.note(format!("restore {id} {row:?}"));
        Ok(())
    }
    fn get(&self, _tx: TxId, _id: RowId) -> Result<Option<Row>> {
        Ok(None)
    }
    fn scan(self: Arc<Self>, _tx: TxId, _columns: &[usize]) -> Rows {
        Box::new(std::iter::empty())
    }
    fn row_count(&self) -> usize {
        0
    }
}

impl Index for Recorder {
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.note(format!("index insert {row} {:?}", key.0));
        Ok(())
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.note(format!("index remove {row} {:?}", key.0));
        Ok(())
    }
    fn rows(&self, _key: &Key) -> Result<Vec<RowId>> {
        Ok(Vec::new())
    }
    fn estimate(&self, _probe: &Probe) -> Option<usize> {
        None
    }
    fn search(&self, _probe: &Probe) -> Result<Vec<(Key, RowId)>> {
        Ok(Vec::new())
    }
}

/// A log holding `limit` bytes of entries, spilling to areas of `spill`,
/// of a budget of `budget` bytes.
fn log(spill: Option<Arc<dyn Spill>>, (limit, budget): (usize, usize)) -> UndoLog {
    UndoLog::new(UndoSpace { limit, batch: 4096, areas: Some(areas(spill, budget)), types: TypeRegistry::default() })
}

/// One entry of each kind, of `table` and `index`.
fn entries(table: &Arc<Recorder>, index: &Arc<Recorder>) -> Vec<UndoEntry> {
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let storage: Arc<dyn TableStorage> = Arc::<Recorder>::clone(table);
    let ix: Arc<dyn Index> = Arc::<Recorder>::clone(index);
    let row = vec![Value::Char("x".into()), Value::Null];
    vec![
        UndoEntry::Inserted { table: name.clone(), storage: Arc::clone(&storage), id: 1 },
        UndoEntry::Deleted { table: name.clone(), storage: Arc::clone(&storage), id: 2, row: row.clone() },
        UndoEntry::Updated { table: name.clone(), storage, id: 3, old: row.clone() },
        UndoEntry::IndexInserted { table: name.clone(), index: Arc::clone(&ix), key: Key(row.clone()), id: 4 },
        UndoEntry::IndexRemoved { table: name, index: ix, key: Key(row), id: 5 },
    ]
}

/// The entries of `entries`, with a log holding the bytes of the largest
/// alone, so each two go to one run: runs of the first two and the next
/// two, and the last held.
fn pushed(spill: Arc<dyn Spill>, entries: Vec<UndoEntry>) -> UndoLog {
    let limit = entries.iter().map(ListItem::bytes).max().unwrap_or_default();
    let mut log = log(Some(spill), (limit, usize::MAX));
    for entry in entries {
        log.push(entry).unwrap();
    }
    log
}

/// Verifies: [X3.135-1986 §7.3 GR3b i, §8.9 GR1]; [DESIGN] RULES.md D38:
/// entries of every kind, past the bytes held, are written to runs and
/// read back to undo them, newest first: those from a place inside a run
/// on, the run read back and the entries before it held; then the rest,
/// as were they never written.
#[test]
fn entries_spilled_are_undone_as_held_ones() {
    let calls = Calls::default();
    let (table, index) = (Arc::new(Recorder(Arc::clone(&calls))), Arc::new(Recorder(Arc::clone(&calls))));
    let mut spilled = pushed(Arc::new(Held { broken: false }), entries(&table, &index));
    assert_eq!(spilled.list.len(), 5);
    assert!(spilled.list.held() < entries(&table, &index).iter().map(ListItem::bytes).sum(), "some written");
    assert!(spilled.take_from(3).unwrap().apply(9).is_empty());
    assert_eq!(spilled.list.len(), 3);
    assert_eq!(spilled.mark(), 3);
    assert!(spilled.take_from(0).unwrap().apply(9).is_empty());
    let row = "[Char(\"x\"), Null]";
    let expected = [
        format!("index insert 5 {row}"),
        format!("index remove 4 {row}"),
        format!("update 3 {row}"),
        format!("restore 2 {row}"),
        "delete 1".to_owned(),
    ];
    assert_eq!(*lock_builtin(&calls), expected);
}

/// Verifies: [DESIGN] RULES.md D38, R-ERR-1: an area the budget refuses is
/// SQLCODE -918, the entry held all the same and undone; a run that cannot
/// be read back, to part the entries from a place inside it, is an error
/// that takes none of them; undone, it fails every table the log names,
/// the entries held undone all the same.
#[test]
fn a_refused_area_or_a_lost_run_is_reported() {
    let calls = Calls::default();
    let (table, index) = (Arc::new(Recorder(Arc::clone(&calls))), Arc::new(Recorder(Arc::clone(&calls))));
    let mut refused = log(None, (1, 0));
    let [first, ..] = <[UndoEntry; 5]>::try_from(entries(&table, &index)).ok().unwrap();
    assert_eq!(refused.push(first).err().map(|e| e.code), Some(Code::Resources));
    assert!(refused.take_from(0).unwrap().apply(9).is_empty());
    assert_eq!(*lock_builtin(&calls), ["delete 1".to_owned()]);
    let mut lost = pushed(Arc::new(Held { broken: true }), entries(&table, &index));
    assert_eq!(lost.take_from(3).err().map(|e| e.code), Some(Code::Io), "a run cut across");
    assert_eq!(lost.list.len(), 5, "none taken");
    let failures = lost.take_from(0).unwrap().apply(9);
    assert!(!failures.is_empty() && failures.iter().all(|f| f.table.is_some() && f.error.code == Code::Io));
    assert_eq!(lock_builtin(&calls).last().map(String::as_str), Some("index insert 5 [Char(\"x\"), Null]"), "the entry held undone");
}

/// Verifies: [DESIGN] R-ERR-1: a record not of an entry of its target, or
/// naming none, is SQLCODE -916.
#[test]
fn records_not_of_entries_are_corrupt() {
    let calls = Calls::default();
    let table: Arc<dyn TableStorage> = Arc::new(Recorder(calls));
    let targets = [Target::Storage(QualifiedName { auth: "A".into(), ident: "T".into() }, table)];
    let record = |kind: u8, target: u32| [vec![kind], target.to_le_bytes().to_vec(), 7_u64.to_le_bytes().to_vec()].concat();
    assert!(entry_of((&targets, &TypeRegistry::default()), &record(0, 0)).is_ok());
    assert_eq!(
        entry_of((&targets, &TypeRegistry::default()), &record(3, 0)).err().map(|e| e.code),
        Some(Code::Corrupt),
        "an index's, of a table"
    );
    assert_eq!(entry_of((&targets, &TypeRegistry::default()), &record(0, 1)).err().map(|e| e.code), Some(Code::Corrupt), "no such target");
}
