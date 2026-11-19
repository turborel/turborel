use std::sync::Arc;

use super::numbered::LEAST_DEAD;
use super::*;
use crate::memory::MemoryBudget;
use crate::value::{Decimal, Spelled, Value};

/// An INTEGER column.
fn int() -> DataType {
    DataType::Exact { precision: 9, scale: 0, spelled: Spelled::Numeric }
}

fn key(v: u64) -> Key {
    Key(vec![Value::Exact(Decimal::new(i128::from(v), 0))])
}

/// An index of one INTEGER key column of `method`.
fn index(method: &LabelsMethod) -> Arc<dyn Index> {
    Arc::from(method.create(&[int()]).unwrap())
}

/// The rows the index numbers now, in use.
fn live(ix: &Arc<dyn Index>) -> Vec<RowId> {
    let set = Arc::clone(ix).members().unwrap().members().unwrap().unwrap();
    (0..u32::try_from(set.len()).unwrap()).filter(|&id| set.holds(id)).map(|id| set.row(id).unwrap()).collect()
}

/// Verifies: [DESIGN] RULES.md D149, D34: rows are numbered in the order
/// of the rows as the numbering is made of the tree, then as written; a
/// row keeps its number while a version holds an entry, and its last
/// taken back leaves the number unused (normal range).
#[test]
fn rows_are_numbered_as_written_and_taken_back() {
    let method = LabelsMethod::in_memory();
    let ix = index(&method);
    for row in [30, 10, 20] {
        ix.insert(key(row), row).unwrap();
    }
    assert_eq!(live(&ix), [10, 20, 30]);
    ix.insert(key(5), 5).unwrap();
    ix.insert(key(11), 10).unwrap();
    assert_eq!(live(&ix), [10, 20, 30, 5]);
    ix.remove(&key(10), 10).unwrap();
    assert_eq!(live(&ix), [10, 20, 30, 5], "a version of row 10 still holds an entry");
    ix.remove(&key(11), 10).unwrap();
    ix.remove(&key(99), 20).unwrap();
    assert_eq!(live(&ix), [20, 30, 5]);
    let set = Arc::clone(&ix).members().unwrap().members().unwrap().unwrap();
    assert_eq!((set.id(5), set.id(10), set.id(30)), (Some(3), None, Some(2)));
    ix.insert(key(10), 10).unwrap();
    assert_eq!(live(&ix), [10, 20, 30, 5], "its number given back");
}

/// Verifies: [DESIGN] RULES.md D149: maintenance numbers the rows again
/// once more than an eighth of the numbers, and more than LEAST_DEAD,
/// are unused, so they are dense again; not before (boundaries).
#[test]
fn maintenance_numbers_the_rows_again_once_many_are_gone() {
    let method = LabelsMethod::in_memory();
    let ix = index(&method);
    let rows = u64::try_from(LEAST_DEAD).unwrap().saturating_add(1);
    for row in 0..rows.saturating_mul(2) {
        ix.insert(key(row), row).unwrap();
    }
    assert_eq!(live(&ix).len(), usize::try_from(rows.saturating_mul(2)).unwrap());
    for row in 0..rows.saturating_sub(1) {
        ix.remove(&key(row), row).unwrap();
    }
    assert!(!ix.maintenance_due(), "{LEAST_DEAD} unused at most");
    ix.remove(&key(rows.saturating_sub(1)), rows.saturating_sub(1)).unwrap();
    assert!(ix.maintenance_due());
    assert!(ix.maintain().unwrap());
    assert!(!ix.maintenance_due());
    let set = Arc::clone(&ix).members().unwrap().members().unwrap().unwrap();
    assert_eq!(set.len(), usize::try_from(rows).unwrap());
    assert_eq!(set.id(rows), Some(0));
}

/// Verifies: [DESIGN] RULES.md D149, D25: the index reopened of its
/// locator holds its entries, given by a search of them all and by key,
/// and numbers its rows of them (normal range); a locator of another
/// format is refused (robustness).
#[test]
fn an_index_reopened_of_its_locator_holds_its_rows() {
    let method = LabelsMethod::in_memory();
    let ix = index(&method);
    for row in [3_u64, 1, 2] {
        ix.insert(key(row.saturating_mul(10)), row).unwrap();
    }
    let opened: Arc<dyn Index> = Arc::from(method.open(&[int()], &ix.locator()).unwrap().unwrap());
    let all: Vec<(Key, RowId)> = Arc::clone(&opened).search(&Probe::All, KeyOrder::Ascending).map(Result::unwrap).collect();
    assert_eq!(all, vec![(key(10), 1), (key(20), 2), (key(30), 3)]);
    assert_eq!(opened.rows(&key(20)).unwrap(), vec![2]);
    assert_eq!(Arc::clone(&opened).search(&Probe::Compare { op: CompOp::Eq, value: Value::Null }, KeyOrder::Ascending).count(), 0);
    assert_eq!(live(&opened), [1, 2, 3]);
    assert!(method.open(&[int()], &[1, 2]).is_err());
}

/// Verifies: [DESIGN] RULES.md D149, D38: a numbering the memory budget
/// refuses is not made, so the index gives no members and the table is
/// read instead, until as many rows were written as it held (robustness).
#[test]
fn a_refused_numbering_gives_no_members() {
    let method = LabelsMethod::in_memory().with_budget(MemoryBudget::new(1024));
    let ix = index(&method);
    ix.insert(key(1), 1).unwrap();
    assert!(Arc::clone(&ix).members().unwrap().members().unwrap().is_none());
    assert!(!ix.maintenance_due());
    for row in 2..=u64::try_from(LEAST_DEAD).unwrap().saturating_add(1) {
        ix.insert(key(row), row).unwrap();
    }
    assert!(ix.maintenance_due());
}
