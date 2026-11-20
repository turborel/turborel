//! Unit tests of late columns (`late.rs`).

use std::sync::Mutex;

use super::*;
use crate::error::{Code, SqlError};
use crate::storage::Ints;

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
    assert_eq!(shown(&late.resolved(Some(&[1, 3])).unwrap()), ["NULL", "50", "NULL", "110"]);
    assert_eq!(tens.asked.lock().unwrap().pop(), Some(vec![5, 11]), "the places kept alone");
    let joined = late.gathered(&[Some(3), None, Some(0), Some(3)]);
    assert_eq!(joined.len(), 4);
    assert_eq!(shown(&joined.resolved(None).unwrap()), ["110", "NULL", "20", "110"]);
    assert_eq!(tens.asked.lock().unwrap().pop(), Some(vec![2, 11]), "each place once, ascending");
    let twice = joined.gathered(&[Some(2), Some(1)]);
    assert_eq!(shown(&twice.resolved(Some(&[0, 1])).unwrap()), ["20", "NULL"]);
    assert_eq!(shown(&Column::Late(late.clone())), ["20", "50", "70", "110"], "read before it is resolved");
    let mut batch = Batch { columns: vec![Column::Late(late), Column::from(vec![Value::Bool(true); 4])], rows: 4 };
    assert!(batch.has_late());
    batch.resolve(Some(&[0])).unwrap();
    assert!(!batch.has_late());
    assert_eq!(shown(&batch.columns[0]), ["20", "NULL", "NULL", "NULL"]);
    let damaged = Late::new(source, 0, vec![1, 99].into());
    assert!(damaged.resolved(None).is_err());
    assert!(damaged.resolved(Some(&[0])).is_ok(), "the damaged place not read");
}
