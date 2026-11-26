//! Unit tests of late columns (`late.rs`).

use std::sync::Mutex;

use super::*;
use crate::error::{Code, SqlError};
use crate::storage::{Ints, NO_ROW};

/// A source whose value of a place is ten times it, the places it was
/// asked to decode noted; it fails if asked for place 99.
#[derive(Default)]
struct Tens {
    asked: Mutex<Vec<Vec<u32>>>,
}

impl LateSource for Tens {
    fn decode(&self, _column: usize, positions: &[u32]) -> Result<Column> {
        if positions.contains(&99) {
            return Err(SqlError::new(Code::Corrupt, "a damaged page"));
        }
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(positions.to_vec());
        }
        Ok(Column::Ints(Ints::new(positions.iter().map(|&p| i64::from(p).saturating_mul(10)).collect(), 0, None)))
    }
}

/// The values of `column` as text.
fn shown(column: &Column) -> Vec<String> {
    column.values().iter().map(ToString::to_string).collect()
}

/// Verifies: [DESIGN] RULES.md D361, late columns: decoded of the rows
/// kept alone, the others null, each place once; gathered by a join as
/// places, a row taken twice and one paired with none, decoded of them;
/// read before it is resolved, decoded whole; a failure of its source
/// raised as it is resolved.
#[test]
fn late_columns_are_decoded_of_the_rows_kept() {
    let tens = Arc::new(Tens::default());
    let source: Arc<dyn LateSource> = Arc::<Tens>::clone(&tens);
    let late = Late::new(Arc::clone(&source), 3, vec![2, 5, 7, 11].into());
    assert_eq!(late.len(), 4);
    assert_eq!(shown(&late.resolved(Some(&[1, 3]), &crate::kernels::Portable).unwrap()), ["NULL", "50", "NULL", "110"]);
    assert_eq!(tens.asked.lock().unwrap().pop(), Some(vec![5, 11]), "the places kept alone");
    let joined = late.gathered(&[Some(3), None, Some(0), Some(3)]);
    assert_eq!(joined.len(), 4);
    assert_eq!(shown(&joined.resolved(None, &crate::kernels::Portable).unwrap()), ["110", "NULL", "20", "110"]);
    assert_eq!(tens.asked.lock().unwrap().pop(), Some(vec![2, 11]), "each place once, ascending");
    let twice = joined.gathered(&[Some(2), Some(1)]);
    assert_eq!(shown(&twice.resolved(Some(&[0, 1]), &crate::kernels::Portable).unwrap()), ["20", "NULL"]);
    assert_eq!(shown(&Column::Late(late.clone())), ["20", "50", "70", "110"], "read before it is resolved");
    let mut batch = Batch { columns: vec![Column::Late(late), Column::from(vec![Value::Bool(true); 4])], rows: 4 };
    assert!(batch.has_late());
    batch.resolve(Some(&[0]), &crate::kernels::Portable).unwrap();
    assert!(!batch.has_late());
    assert_eq!(shown(&batch.columns[0]), ["20", "NULL", "NULL", "NULL"]);
    let damaged = Late::new(source, 0, vec![1, 99].into());
    assert!(damaged.resolved(None, &crate::kernels::Portable).is_err());
    assert!(damaged.resolved(Some(&[0]), &crate::kernels::Portable).is_ok(), "the damaged place not read");
}

/// Verifies: [DESIGN] RULES.md D1162: of a column no join gathered, the
/// rows kept, ascending and each once, are decoded of their places and
/// each given its own, as the rows of any order are: the first and last
/// rows, rows past its end left out, and the same rows given out of order,
/// which take the general path. MC/DC of the rows taken in order: no
/// picks and rows ascending (taken) against a join's picks, and rows
/// given twice or out of order.
#[test]
fn rows_kept_in_order_are_decoded_of_their_places_alone() {
    let tens = Arc::new(Tens::default());
    let source: Arc<dyn LateSource> = Arc::<Tens>::clone(&tens);
    let late = Late::new(source, 0, vec![1, 4, 6, 8, 9].into());
    let resolved = |rows: &[u32]| shown(&late.resolved(Some(rows), &crate::kernels::Portable).unwrap());
    assert_eq!(resolved(&[0, 2, 4, 7]), ["10", "NULL", "60", "NULL", "90"]);
    assert_eq!(tens.asked.lock().unwrap().pop(), Some(vec![1, 6, 9]), "the places of the rows kept, row 7 past its end");
    assert_eq!(resolved(&[4, 0, 2]), ["10", "NULL", "60", "NULL", "90"], "out of order, alike");
    assert_eq!(resolved(&[3, 3]), ["NULL", "NULL", "NULL", "80", "NULL"], "a row given twice");
    assert_eq!(resolved(&[]), ["NULL", "NULL", "NULL", "NULL", "NULL"]);
    let joined = late.gathered(&[Some(4), Some(1)]);
    assert_eq!(shown(&joined.resolved(Some(&[0, 1]), &crate::kernels::Portable).unwrap()), ["90", "40"], "a join's picks");
}

/// Verifies: [DESIGN] RULES.md D1096: a built operand's late column is its
/// rows of the operand, gathered through a join as rows, a null for none,
/// and resolved by one gather of the rows kept alone, the others null; read
/// before it is resolved, it gathers its own rows, never the operand's;
/// a vector late itself is never made a built one; and once every batch is
/// resolved the operand is held by its join alone.
#[test]
fn a_built_late_column_is_gathered_of_its_own_rows() {
    let texts: Vec<Value> = ["a", "b", "c", "d", "e"].iter().map(|t| Value::Char((*t).into())).collect();
    let right = Arc::new(Batch { columns: vec![Column::from(texts), Column::Ints(Ints::new(vec![1, 2, 3, 4, 5], 0, None))], rows: 5 });
    let rows: Arc<[u32]> = vec![4, 0, NO_ROW, 2].into();
    let late = Late::built(&right, 0, &rows).unwrap();
    assert!(late.is_built());
    assert_eq!(late.len(), 4);
    assert_eq!(shown(&late.resolved(Some(&[0, 3]), &crate::kernels::Portable).unwrap()), ["e", "NULL", "NULL", "c"]);
    let joined = late.gathered(&[Some(3), None, Some(1), Some(3)]);
    assert_eq!(shown(&joined.resolved(None, &crate::kernels::Portable).unwrap()), ["c", "NULL", "a", "c"]);
    assert_eq!(joined.made().map(|c| c.values().len()), Some(4), "read before, its own rows alone");
    let ints = Late::built(&right, 1, &rows).unwrap();
    assert_eq!(shown(&ints.resolved(None, &crate::kernels::Portable).unwrap()), ["5", "1", "NULL", "3"]);
    assert!(Late::built(&right, 2, &rows).is_none(), "no such vector");
    let scanned: Arc<dyn LateSource> = Arc::new(Tens::default());
    let inner = Arc::new(Batch { columns: vec![Column::Late(Late::new(scanned, 0, vec![1].into()))], rows: 1 });
    assert!(Late::built(&inner, 0, &rows).is_none(), "a late vector is not wrapped");
    let mut batch = Batch { columns: vec![Column::Late(late), Column::Late(joined), Column::Late(ints)], rows: 4 };
    assert_eq!(Arc::strong_count(&right), 4);
    batch.resolve_built(Some(&[1]), &crate::kernels::Portable).unwrap();
    assert!(!batch.has_late());
    assert_eq!(shown(&batch.columns[0]), ["NULL", "a", "NULL", "NULL"]);
    assert_eq!(Arc::strong_count(&right), 1, "the operand held by its join alone");
}

/// Verifies: [DESIGN] RULES.md D1096: a scan's late columns of two
/// neighbouring batches each decode their own places: one read whole
/// before it is resolved never widens what the other asks, and a batch's
/// built late columns resolved leave its scan's late.
#[test]
fn neighbouring_late_batches_decode_their_own_rows() {
    let tens = Arc::new(Tens::default());
    let source: Arc<dyn LateSource> = Arc::<Tens>::clone(&tens);
    let first = Late::new(Arc::clone(&source), 0, vec![1, 2, 3].into());
    let second = Late::new(source, 0, vec![4, 5, 6].into());
    assert_eq!(first.made().map(|c| c.values().len()), Some(3));
    assert_eq!(tens.asked.lock().unwrap().pop(), Some(vec![1, 2, 3]));
    let _ = second.resolved(Some(&[1]), &crate::kernels::Portable).unwrap();
    assert_eq!(tens.asked.lock().unwrap().pop(), Some(vec![5]), "its own row alone");
    let mut batch = Batch { columns: vec![Column::Late(second)], rows: 3 };
    batch.resolve_built(None, &crate::kernels::Portable).unwrap();
    assert!(batch.has_late(), "a scan's late column stays late");
    assert!(tens.asked.lock().unwrap().is_empty(), "nothing decoded");
}
