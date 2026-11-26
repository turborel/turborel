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

/// A source of texts: of a place, its number spelled with as many
/// letters, null of place 3; of place 0 an empty text.
struct Spelled;

impl LateSource for Spelled {
    fn decode(&self, _column: usize, positions: &[u32]) -> Result<Column> {
        let (mut text, mut ends, mut nulls) = (String::new(), Vec::new(), Vec::new());
        for &p in positions {
            if p != 3 {
                text.push_str(&"é".repeat(usize::try_from(p).unwrap()));
            }
            ends.push(text.len());
            nulls.push(p == 3);
        }
        Ok(Column::Texts(Texts::new(text, ends, Some(crate::storage::Mask::from_bools(&nulls)))))
    }
}

/// Verifies: [DESIGN] RULES.md D1169: of a column no join gathered, the
/// texts of the rows kept, ascending and each once, are laid out over the
/// rows as decoded, the others null: a null kept null, an empty text kept
/// empty, the first and last rows, a row past the end left out; and as
/// the rows of any order give them.
#[test]
fn texts_of_rows_kept_in_order_are_laid_out_as_decoded() {
    let late = Late::new(Arc::new(Spelled), 0, vec![0, 2, 3, 5, 1].into());
    let resolved = |rows: &[u32]| shown(&late.resolved(Some(rows), &crate::kernels::Portable).unwrap());
    let laid = late.resolved(Some(&[0, 1, 2, 4, 9]), &crate::kernels::Portable).unwrap();
    let Column::Texts(texts) = &laid else { panic!("texts") };
    assert_eq!(texts.text, "ééé", "the text as decoded, none copied twice");
    assert_eq!(shown(&laid), ["", "éé", "NULL", "NULL", "é"]);
    assert_eq!(resolved(&[4, 0, 1, 2]), shown(&laid), "out of order, alike");
    assert_eq!(resolved(&[3]), ["NULL", "NULL", "NULL", "ééééé", "NULL"]);
    assert_eq!(resolved(&[]), ["NULL"; 5]);
}
