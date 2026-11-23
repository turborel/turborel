//! Unit tests of what a statement's writes resolve once (RULES.md D872,
//! R-TEST-1): what a write finds done in its transaction, which no API
//! shows, and that one `Writing` held across statements writes as a table's
//! name does.

use std::mem;
use std::sync::Arc;

use super::{Columns, Entries, read_columns};
use crate::catalog::{Catalog, ColumnDesc, IndexDesc, MemoryCatalog, QualifiedName, TableDesc};
use crate::db::State;
use crate::engine::rows::{RowStore, TableOptions};
use crate::error::{Code, SqlError};
use crate::storage::{Row, StorageEngine, TableStorage};
use crate::txn::{Concurrency, TableLock, Txn, Undo};
use crate::value::{DataType, Decimal, Spelled, Value};

/// A database with table A.T (K), in the row store, and a transaction open.
fn database() -> (State, Txn, QualifiedName) {
    database_of(&["K"])
}

/// As `database`, of a table of the numeric columns `columns`.
fn database_of(columns: &[&str]) -> (State, Txn, QualifiedName) {
    let name = QualifiedName { auth: "A".into(), ident: "T".into() };
    let column = |column: &&str| ColumnDesc {
        name: (*column).into(),
        data_type: DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric },
        not_null: false,
        default: None,
        domain: None,
        collation: None,
        generated: None,
    };
    let desc = TableDesc {
        periods: Vec::new(),
        typed: None,
        name: name.clone(),
        columns: columns.iter().map(column).collect(),
        constraints: vec![],
        engine: "rowstore".into(),
        options: Vec::new(),
        generation: 0,
        temporary: None,
    };
    let storage: Arc<dyn TableStorage> = Arc::from(RowStore::in_memory(TableOptions::default()).create_table(&desc).unwrap());
    let mut catalog = MemoryCatalog::default();
    catalog.create_schema("A", "A", vec![desc]);
    let mut st = State::with_builtins(Box::new(catalog));
    st.storage.insert(Arc::new(name.clone()), storage);
    st.locks.insert(name.clone(), Arc::new(TableLock::default()));
    let tx = st.begin();
    (st, tx, name)
}

fn row(k: i64) -> Row {
    vec![Value::Exact(Decimal::new(k.into(), 0))]
}

/// An index of `table` on its first two columns, to make `Entries` of.
fn index_of(table: &QualifiedName) -> IndexDesc {
    IndexDesc {
        name: "T_KA".into(),
        table: table.clone(),
        columns: vec![0, 1],
        method: crate::index::BTREE_METHOD.into(),
        unique: false,
        options: Vec::new(),
        collations: Vec::new(),
        nulls: false,
        expressions: Vec::new(),
    }
}

/// A row of the numbers `values`.
fn numbers(values: &[i64]) -> Row {
    values.iter().map(|&v| Value::Exact(Decimal::new(v.into(), 0))).collect()
}

/// Verifies: [DESIGN] RULES.md D872: a write finds its first work done
/// only once the transaction wrote the table, joined its engine and took
/// its statement's savepoint there, and while no table it wrote has had
/// its definitions changed. MC/DC of `Txn::prepared`: all four hold
/// against each taken away alone, not written, not joined, no savepoint of
/// the statement, definitions changed.
#[test]
fn a_write_finds_its_first_work_done_only_while_it_holds() {
    let (st, tx, name) = database();
    let writing = st.writing(&name).unwrap();
    assert!(!tx.prepared(&writing).unwrap(), "nothing written yet");
    st.insert_row_in(&tx, &writing, &row(1)).unwrap();
    assert!(tx.prepared(&writing).unwrap(), "written, joined, its statement's savepoint taken");
    let written = mem::take(&mut tx.state().unwrap().written);
    assert!(!tx.prepared(&writing).unwrap(), "not written");
    tx.state().unwrap().written = written;
    let joined = mem::take(&mut tx.state().unwrap().joined);
    assert!(!tx.prepared(&writing).unwrap(), "not joined");
    tx.state().unwrap().joined = joined;
    let saved = mem::take(&mut tx.state().unwrap().statement_savepoints);
    assert!(!tx.prepared(&writing).unwrap(), "no savepoint of the statement");
    tx.state().unwrap().statement_savepoints = saved;
    assert!(tx.prepared(&writing).unwrap());
    st.lock_of(&name).unwrap().definitions_changed(st.generation.saturating_add(1));
    assert!(!tx.prepared(&writing).unwrap(), "its definitions changed");
    assert_eq!(st.insert_row_in(&tx, &writing, &row(2)).err().map(|e| e.code), Some(Code::Conflict), "refused as by name");
}

/// Verifies: [DESIGN] RULES.md D872, [X3.135-1986 §7.3 GR3b i]: one
/// `Writing` held across statements writes as the table's name does. A
/// statement that fails after writing through it is undone alone, its
/// savepoint taken anew though the transaction wrote the table before;
/// rows updated and deleted through it are undone with it.
#[test]
fn a_writing_held_across_statements_writes_as_the_name_does() {
    let (st, tx, name) = database();
    let writing = st.writing(&name).unwrap();
    let storage = Arc::clone(st.storage_of(&name).unwrap());
    let rows = |by: &Txn| -> Vec<String> {
        let mut rows: Vec<String> = Arc::clone(&storage).scan(by.id, &[0]).map(|r| r.unwrap().1[0].to_string()).collect();
        rows.sort();
        rows
    };
    tx.begin_statement(|| false).unwrap();
    let first = st.insert_row_in(&tx, &writing, &row(1)).unwrap();
    let second = st.insert_row_in(&tx, &writing, &row(2)).unwrap();
    tx.begin_statement(|| false).unwrap();
    st.insert_row_in(&tx, &writing, &row(3)).unwrap();
    st.update_row_in(&tx, &writing, first, &row(10)).unwrap();
    st.fail_statement(&tx, &SqlError::new(Code::Api, "[DESIGN] failed")).unwrap();
    assert_eq!(rows(&tx), ["1", "2"], "the failed statement undone alone");
    tx.begin_statement(|| false).unwrap();
    st.update_row_in(&tx, &writing, first, &row(10)).unwrap();
    st.delete_row_in(&tx, &writing, second).unwrap();
    assert_eq!(rows(&tx), ["10"]);
    st.insert_row_in(&tx, &writing, &row(11)).unwrap();
    st.fail_statement(&tx, &SqlError::new(Code::Api, "[DESIGN] failed")).unwrap();
    assert_eq!(rows(&tx), ["1", "2"]);
    st.commit(&tx).unwrap();
    assert_eq!(Arc::clone(&storage).scan(crate::txn::NO_TRANSACTION, &[0]).count(), 2);
}

/// Verifies: [DESIGN] RULES.md D875: of a statement that said which
/// columns it sets, the key columns it does not set are taken as kept with
/// no value compared, and those it sets are compared; of one that said
/// nothing, every key column is compared. MC/DC of a key column kept: set
/// and alike, set and changed, not set.
#[test]
fn key_columns_a_statement_does_not_set_are_kept_unread() {
    let (st, _tx, name) = database();
    let mut writing = st.writing(&name).unwrap();
    writing.key_columns = 0b011;
    let (old, new) =
        (vec![row(1)[0].clone(), row(2)[0].clone(), row(3)[0].clone()], vec![row(9)[0].clone(), row(2)[0].clone(), row(4)[0].clone()]);
    assert_eq!(writing.kept_columns(&old, &new), 0b010, "nothing said: both key columns compared");
    let third = writing.setting(0b100);
    assert_eq!(third.kept_columns(&old, &new), 0b011, "neither key column set: kept, the first not looked at");
    let first = third.setting(0b001);
    assert_eq!(first.kept_columns(&old, &new), 0b010, "the first set and changed, the second not set");
    let both = first.setting(0b011);
    assert_eq!(both.kept_columns(&old, &old), 0b011, "set and alike");
}

/// Verifies: [DESIGN] RULES.md D875: an update reads of the row it
/// replaces the columns its statement sets, and every column of each
/// index key that reads one of them; a key that reads none set adds
/// nothing. MC/DC of a key's columns read: its key meets the set, against
/// one that misses it. The columns of a mask come in ascending order, the
/// last bit's among them.
#[test]
fn an_update_reads_the_columns_set_and_the_keys_they_meet() {
    assert_eq!(read_columns(0b0001, [0b0011, 0b1100].into_iter()), 0b0011, "the first key meets the set, the second misses it");
    assert_eq!(read_columns(0b0001, [0b1100].into_iter()), 0b0001, "no key meets it");
    assert_eq!(read_columns(0b0101, [0b0011, 0b1100].into_iter()), 0b1111, "both meet it");
    assert_eq!(read_columns(0b0100, std::iter::empty()), 0b0100, "no index");
    assert_eq!(Columns::of(0b1010 | (1 << 63)).to_vec(), [1, 3, 63]);
    assert!(Columns::of(0).is_empty() && Columns::of(u64::MAX).len() == 64, "none, and every one");
    let (st, _tx, name) = database_of(&["K", "A", "B", "C"]);
    let mut writing = st.writing(&name).unwrap();
    assert!(writing.sparse().is_none() && writing.set_columns().is_none(), "nothing said");
    // As of an index on (K, A) and one on C.
    let index = index_of(&name);
    writing.indexes =
        vec![Entries { columns: 0b0011, ..Entries::of(&index, None) }, Entries { columns: 0b1000, ..Entries::of(&index, None) }];
    let set = writing.setting(0b0110);
    assert_eq!(set.set_columns().as_deref(), Some(&[1_usize, 2][..]));
    assert_eq!(set.sparse().map(|s| (s.columns().to_vec(), s.read().to_vec())), Some((vec![1, 2], vec![0, 1, 2])));
}

/// Verifies: [DESIGN] RULES.md D875: the row replaced is read by columns
/// only of a table whose engine keeps versions and undoes its own
/// changes, and only once its statement said what it sets. MC/DC of
/// `Writing::sparse`: said, own undo and versions, against nothing said,
/// an engine undone by the coordinator's log, and one whose isolation the
/// coordinator keeps, each alone refusing.
#[test]
fn a_row_is_read_by_columns_only_where_its_engine_needs_no_more() {
    let (st, _tx, name) = database_of(&["K", "A"]);
    let native = st.writing(&name).unwrap().caps;
    assert!((native.undo, native.concurrency) != (Undo::Log, Concurrency::Coordinator), "the row store's own");
    assert!(st.writing(&name).unwrap().sparse().is_none(), "nothing said");
    assert!(st.writing(&name).unwrap().setting(0b10).sparse().is_some());
    let mut logged = st.writing(&name).unwrap().setting(0b10);
    logged.caps.undo = Undo::Log;
    assert!(logged.sparse().is_none(), "the whole row is kept to put back");
    let mut coordinated = st.writing(&name).unwrap().setting(0b10);
    coordinated.caps.concurrency = Concurrency::Coordinator;
    assert!(coordinated.sparse().is_none(), "each index's old key is made of the row");
}

/// Verifies: [DESIGN] RULES.md D875: an update by a statement that said
/// the columns it sets is given the row it replaced with the columns it
/// reads alone, every other null, and leaves the row as an update that
/// read the row whole leaves it; an update through the same `Writing` that
/// asks for the row whole is given it whole.
#[test]
fn an_update_of_the_columns_set_is_given_those_of_the_row_replaced() {
    let (st, tx, name) = database_of(&["K", "A", "B"]);
    let writing = st.writing(&name).unwrap().setting(0b100);
    let storage = Arc::clone(st.storage_of(&name).unwrap());
    tx.begin_statement(|| false).unwrap();
    let id = st.insert_row_in(&tx, &writing, &numbers(&[1, 2, 3])).unwrap();
    let mut recheck = |_: &Row| Ok(crate::txn::Again::Skip);
    // A row's values as text, a null as `NULL`.
    let shown = |row: Option<Row>| row.map(|row| row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    let old = st.rewrite_row_set(&tx, (&writing, id), &numbers(&[1, 2, 9]), &mut recheck).unwrap();
    assert_eq!(shown(old).as_deref(), Some("NULL,NULL,3"), "the column set alone");
    assert_eq!(shown(storage.get(tx.id, id).unwrap()).as_deref(), Some("1,2,9"));
    let whole = st.rewrite_row_in(&tx, (&writing, id), Some(&numbers(&[1, 2, 10])), &mut recheck).unwrap();
    assert_eq!(shown(whole).as_deref(), Some("1,2,9"), "asked for whole");
    st.commit(&tx).unwrap();
    assert_eq!(shown(storage.get(crate::txn::NO_TRANSACTION, id).unwrap()).as_deref(), Some("1,2,10"));
}

/// Verifies: [DESIGN] RULES.md D917: an update is written by the columns
/// set, its row holding those and its index keys' columns, only where
/// nothing reads another column of it. MC/DC of `Writing::patched`: a
/// statement that said what it sets, of a table read by columns, with
/// nothing checked beside it, no period and every index's key columns
/// known, against each taken away alone: nothing said, an engine undone
/// by the coordinator's log, a check beside the table, a system-time
/// period, and an index whose key columns are not known.
#[test]
fn a_row_is_written_by_the_columns_set_only_where_nothing_reads_another() {
    let (st, _tx, name) = database_of(&["K", "A", "B"]);
    let index = index_of(&name);
    let said = || {
        let mut writing = st.writing(&name).unwrap();
        writing.key_columns = 0b011;
        writing.indexes = vec![Entries { columns: 0b011, ..Entries::of(&index, None) }];
        writing.setting(0b100)
    };
    assert_eq!(said().patched(), Some(0b111), "the columns set and those the keys read");
    assert!(st.writing(&name).unwrap().patched().is_none(), "nothing said");
    let mut logged = said();
    logged.caps.undo = Undo::Log;
    assert!(logged.patched().is_none(), "the whole row is kept to put back");
    let mut checked = said();
    checked.checked = true;
    assert!(checked.patched().is_none(), "a check beside the table reads the row written");
    let mut stamped = said();
    stamped.stamped = true;
    assert!(stamped.patched().is_none(), "a period stamps the row written");
    let mut unknown = said();
    unknown.indexes.push(Entries { columns: 0, ..Entries::of(&index, None) });
    assert!(unknown.patched().is_none(), "an index's key is made of columns not known");
}

/// Verifies: [DESIGN] RULES.md D917, D916: an update whose row holds the
/// values of the columns it sets alone leaves every other column as the
/// table holds it, and is given the row it replaced by the columns it
/// reads; through a `Writing` that may not be written so, here one told
/// no columns, it is refused (SQLCODE -910) and nothing is written.
#[test]
fn an_update_holding_the_columns_set_alone_keeps_every_other() {
    let (st, tx, name) = database_of(&["K", "A", "B"]);
    let writing = st.writing(&name).unwrap().setting(0b100);
    let storage = Arc::clone(st.storage_of(&name).unwrap());
    tx.begin_statement(|| false).unwrap();
    let id = st.insert_row_in(&tx, &writing, &numbers(&[1, 2, 3])).unwrap();
    let mut recheck = |_: &Row| Ok(crate::txn::Again::Skip);
    let shown = |row: Option<Row>| row.map(|row| row.iter().map(ToString::to_string).collect::<Vec<_>>().join(","));
    let set = vec![Value::Null, Value::Null, numbers(&[9])[0].clone()];
    let old = st.rewrite_row_patch(&tx, (&writing, id), &set, &mut recheck).unwrap();
    assert_eq!(shown(old).as_deref(), Some("NULL,NULL,3"), "the column set alone");
    assert_eq!(shown(storage.get(tx.id, id).unwrap()).as_deref(), Some("1,2,9"), "the columns not set are kept");
    let unsaid = st.writing(&name).unwrap();
    assert_eq!(st.rewrite_row_patch(&tx, (&unsaid, id), &set, &mut recheck).err().map(|e| e.code), Some(Code::Api));
    st.commit(&tx).unwrap();
    assert_eq!(shown(storage.get(crate::txn::NO_TRANSACTION, id).unwrap()).as_deref(), Some("1,2,9"));
}
