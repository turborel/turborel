//! Unit tests of the bookkeeping a thread keeps (RULES.md D871, R-TEST-1):
//! which table lock a read or a write takes, which no API shows.

use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::Duration;

use turborel_test_support::Scratch;

use super::super::stamps::tests::table;
use crate::storage::TableStorage;
use crate::sync::lock_builtin;
use crate::txn::ReadMode;
use crate::value::Value;

fn row(text: &str) -> Vec<Value> {
    vec![Value::Char(text.into())]
}

/// Verifies: [DESIGN] RULES.md D871: once a thread has found a
/// transaction's bookkeeping in a table, its next read, write and
/// savepoint there take no lock of the table's map of open transactions:
/// they return though another holds every shard of it.
#[test]
fn bookkeeping_is_found_again_without_the_tables_map() {
    let scratch = Scratch::new("unit-found-again");
    let (_files, table, id) = table(&scratch);
    let table = Arc::new(table);
    let (found, first) = channel();
    let (go, held) = channel::<()>();
    let (done, finished) = channel();
    let session = Arc::clone(&table);
    std::thread::spawn(move || {
        session.begin(10, 9).unwrap();
        assert!(session.get(10, id).unwrap().is_some());
        found.send(()).unwrap();
        held.recv().unwrap();
        assert!(session.get(10, id).unwrap().is_some());
        session.update(10, id, row("new  ")).unwrap();
        done.send(session.savepoint(10)).unwrap();
    });
    first.recv().unwrap();
    let shards = table.versions.open.held();
    go.send(()).unwrap();
    let changes = finished.recv_timeout(Duration::from_secs(5));
    drop(shards);
    assert_eq!(changes.ok(), Some(1), "the thread waited on the table's map");
}

/// Verifies: [DESIGN] RULES.md D871: what a thread keeps is given only to
/// the transaction it is of, in the table it is of, while it is open
/// there. MC/DC of giving it: (same transaction, same table, open) given,
/// against another table, another transaction, and the transaction ended,
/// each refused or found anew.
#[test]
fn bookkeeping_kept_is_of_its_transaction_and_table_while_open() {
    let (first, second) = (Scratch::new("unit-found-first"), Scratch::new("unit-found-second"));
    let ((_files, table, id), (_others, other, other_id)) = (table(&first), table(&second));
    table.begin(10, 9).unwrap();
    table.update(10, id, row("mine ")).unwrap();
    assert_eq!(table.savepoint(10), 1, "kept by the thread, and found again");
    assert!(other.update(10, other_id, row("other")).is_err(), "not begun in the other table");
    assert!(table.update(11, id, row("other")).is_err(), "another transaction has not begun");
    table.commit(10, 10);
    table.settle(10);
    assert!(table.update(10, id, row("late ")).is_err(), "ended here, though the thread still keeps it");
    table.begin(10, 10).unwrap();
    assert_eq!(table.savepoint(10), 0, "begun again, its bookkeeping is new");
    table.update(10, id, row("again")).unwrap();
    assert_eq!(table.savepoint(10), 1);
}

/// Verifies: [DESIGN] RULES.md D954: a transaction whose reads are not
/// checked reads with no lock of its bookkeeping taken, at the snapshot
/// last given it, which never goes back, and records nothing; one whose
/// reads are checked records each under the lock. MC/DC of the lock:
/// (not checked) none taken, the read returning while another holds it,
/// against (checked), the read recorded; and of what is noted as its mode
/// changes: checked, not checked, checked again, and preserving.
#[test]
fn a_read_not_checked_takes_no_lock_of_its_bookkeeping() {
    let scratch = Scratch::new("unit-found-unchecked");
    let (_files, table, id) = table(&scratch);
    let table = Arc::new(table);
    table.begin(10, 9).unwrap();
    let opened = table.versions.open.get(10).unwrap();
    assert_eq!(opened.unchecked(), None, "begun with its reads checked");
    table.read_mode(10, ReadMode { snapshot: 9, checked: false }).unwrap();
    assert_eq!(opened.unchecked(), Some(9));
    let held = lock_builtin(&opened);
    let (done, finished) = channel();
    let session = Arc::clone(&table);
    let reader = std::thread::spawn(move || {
        let found = session.get(10, id).unwrap().is_some();
        session.note_read(10, || panic!("not checked: no read is made to record")).unwrap();
        done.send((found, session.reader(10).is_ok())).unwrap();
    });
    let read = finished.recv_timeout(Duration::from_secs(5));
    drop(held);
    reader.join().unwrap();
    assert_eq!(read.ok(), Some((true, true)), "the read waited on the bookkeeping's lock");
    assert!(!lock_builtin(&opened).reads.any(), "nothing recorded");
    table.read_mode(10, ReadMode { snapshot: 12, checked: false }).unwrap();
    table.read_mode(10, ReadMode { snapshot: 5, checked: false }).unwrap();
    assert_eq!(opened.unchecked(), Some(12), "a later snapshot, never an earlier one");
    table.read_mode(10, ReadMode { snapshot: 12, checked: true }).unwrap();
    assert_eq!(opened.unchecked(), None, "checked again");
    assert!(table.get(10, id).unwrap().is_some());
    assert!(lock_builtin(&opened).reads.rows.contains_key(&id), "recorded under the lock");
    table.preserving(10).unwrap();
    assert_eq!(opened.unchecked(), Some(12), "preserving, its reads are not checked");
    assert!(!lock_builtin(&opened).reads.any());
}
