//! Unit tests: the runs of rows of one key told of each kind of vector
//! (RULES.md R-TEST-1, D1323).

use std::sync::Arc;

use super::*;
use crate::storage::mask::mask_of;
use crate::storage::{Coded, Datetimes, Floats, Ints};
use crate::value::datetime::Moment;

/// A column of integers of `units`, a null of none.
fn ints(units: &[Option<i64>]) -> Column {
    let nulls: Vec<bool> = units.iter().map(Option::is_none).collect();
    // A null's unit is that of the row before it, so only its null tells
    // it apart.
    let mut held = 0_i64;
    let filled = units.iter().map(|u| {
        held = u.unwrap_or(held);
        held
    });
    Column::Ints(Ints::new(filled.collect(), 0, mask_of(&nulls)))
}

/// A column of character strings of `texts`, each held as its own text,
/// a null of none.
fn texts(texts: &[Option<&str>]) -> Column {
    let (mut text, mut ends) = (String::new(), Vec::new());
    for t in texts {
        text.push_str(t.unwrap_or(""));
        ends.push(text.len());
    }
    let nulls: Vec<bool> = texts.iter().map(Option::is_none).collect();
    Column::Texts(Texts::new(text, ends, mask_of(&nulls)))
}

/// A batch of `columns`, of `rows` rows.
fn batch(columns: Vec<Column>, rows: usize) -> Batch {
    Batch { columns, rows }
}

/// Verifies: [DESIGN] (RULES.md D1323) [ISO/IEC 9075-2:2023 §7.13 GR2]
/// of two columns of integers a run ends where either column's value
/// changes, so a row alike the row before it in one column alone begins a
/// run, and the last run ends at the rows' count. A null is alike a null
/// and unlike a number, though its place holds the number before it. A
/// batch of one row is one run, and a grouping by one of the columns
/// tells that column's runs.
#[test]
fn a_run_ends_where_any_column_changes() {
    let g = ints(&[Some(1), Some(1), Some(1), Some(2), Some(2), None, None, Some(2)]);
    let u = ints(&[Some(7), Some(7), Some(8), Some(8), Some(8), Some(8), Some(8), Some(8)]);
    let both = batch(vec![g, u], 8);
    assert_eq!(ends(&both, &[0, 1], None), Some(vec![2, 3, 5, 7, 8]));
    assert_eq!(ends(&both, &[1, 0], None), Some(vec![2, 3, 5, 7, 8]));
    assert_eq!(ends(&both, &[0], None), Some(vec![3, 5, 7, 8]));
    assert_eq!(ends(&both, &[1], None), Some(vec![2, 8]));
    let one = batch(vec![ints(&[Some(4)])], 1);
    assert_eq!(ends(&one, &[0], None), Some(vec![1]));
}

/// Verifies: [DESIGN] (RULES.md D1323) of rows selected, each is
/// compared with the row given before it, not the batch's row before it:
/// two rows given one after the other with a row of another key between
/// them in the batch are one run, and two rows next to each other in the
/// batch are two runs where the selection gives another between them in
/// its own order.
#[test]
fn selected_rows_are_compared_in_the_order_given() {
    let g = ints(&[Some(1), Some(9), Some(1), Some(1), None, Some(3), None]);
    let rows = batch(vec![g], 7);
    assert_eq!(ends(&rows, &[0], Some(&[0, 2, 3, 5])), Some(vec![3, 4]));
    assert_eq!(ends(&rows, &[0], Some(&[4, 6, 0])), Some(vec![2, 3]));
    assert_eq!(ends(&rows, &[0], Some(&[0, 1, 2])), Some(vec![1, 2, 3]));
    // A row given past the vector tells no run.
    assert_eq!(ends(&rows, &[0], Some(&[0, 7])), None);
    assert_eq!(ends(&rows, &[0], Some(&[])), None);
}

/// Verifies: [DESIGN] (RULES.md D1323) [ISO/IEC 9075-2:2023 §4.2.2,
/// §7.13 GR2] character strings are alike as their octets are the same:
/// a string and the same string with a trailing space are two runs,
/// which the table of groups then compares as the grouping does; a null
/// is unlike the empty string and alike a null; of strings none null,
/// two of one length are alike only as their octets are. Codes of a dictionary
/// are alike as the codes are, and datetimes as their instants.
#[test]
fn texts_codes_and_datetimes_tell_their_runs() {
    let plain = texts(&[Some("a"), Some("a"), Some("a "), Some(""), None, None, Some(""), Some("b")]);
    assert_eq!(ends(&batch(vec![plain], 8), &[0], None), Some(vec![2, 3, 4, 6, 7, 8]));
    // None null and every row given: octets against octets, lengths first.
    let dense = texts(&[Some("a"), Some("a"), Some("a "), Some(""), Some(""), Some("b"), Some("c"), Some("ab"), Some("ab")]);
    assert_eq!(ends(&batch(vec![dense], 9), &[0], None), Some(vec![2, 3, 5, 6, 7, 9]));
    let entries = Arc::new(Texts::new("ab".to_owned(), vec![1, 2], None));
    let codes = vec![0_u32, 0, 1, 1, 0, 0];
    let nulls = mask_of(&[false, false, false, false, false, true]);
    let by_code = Column::Texts(Texts::coded(Coded { codes, entries }, nulls));
    assert_eq!(ends(&batch(vec![by_code], 6), &[0], None), Some(vec![2, 4, 5, 6]));
    let days = Datetimes::new(vec![5, 5, 6, 6, 6], None, (Moment::Date, false, 0), None);
    assert_eq!(ends(&batch(vec![Column::Datetimes(days)], 5), &[0], None), Some(vec![2, 5]));
}

/// Verifies: [DESIGN] (RULES.md D1323) a grouping one of whose columns
/// tells no run by its vector tells none, so every row is looked up: of
/// approximate numbers, whose zeros of both signs are one group; of no
/// column; of no row; of a column the batch does not hold; and of a
/// vector shorter than the batch.
#[test]
fn a_column_that_tells_no_run_tells_none() {
    let floats = Column::Floats(Floats::new(vec![0.0_f64, -0.0_f64, 1.0_f64], None));
    let rows = batch(vec![ints(&[Some(1), Some(1), Some(2)]), floats], 3);
    assert_eq!(ends(&rows, &[0, 1], None), None);
    assert_eq!(ends(&rows, &[1], None), None);
    assert_eq!(ends(&rows, &[], None), None);
    assert_eq!(ends(&rows, &[2], None), None);
    assert_eq!(ends(&batch(vec![ints(&[])], 0), &[0], None), None);
    assert_eq!(ends(&batch(vec![ints(&[Some(1), Some(1)])], 3), &[0], None), None);
    assert_eq!(ends(&batch(vec![texts(&[Some("a")])], 2), &[0], None), None);
}
