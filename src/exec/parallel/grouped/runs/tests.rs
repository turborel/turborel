//! Unit tests of the runs a grouping's partitions give (RULES.md D1125):
//! runs of rows and of batches are given as one, in the order of their
//! first rows, each group's row the values its batch holds of it.

use super::*;
use crate::storage::{Column, Ints, Mask, Texts};
use crate::value::{Decimal, Value};

/// The exact number `n`.
fn exact(n: i64) -> Value {
    Value::Exact(Decimal::new(i128::from(n), 0))
}

/// A run of three groups as a batch, of first rows 0, 3 and 9: a column
/// of integers, the second null, and one of texts.
fn columns() -> Run {
    let nulls = Mask::from_bools(&[false, true, false]);
    let batch = Batch {
        columns: vec![
            Column::Ints(Ints::new(vec![10, 0, 30], 0, Some(nulls))),
            Column::Texts(Texts::new("abcdd ".to_owned(), vec![1, 3, 6], None)),
        ],
        rows: 3,
    };
    Run::Columns(batch, vec![0, 3, 9])
}

/// A run of two groups as rows, of first rows 1 and 5.
fn rows() -> Run {
    Run::Rows(vec![(1, vec![exact(1), Value::Char("x".into())]), (5, vec![exact(5), Value::Char("y".into())])])
}

/// Verifies: [DESIGN] (RULES.md D1125, D293) the groups of runs of rows
/// and of batches, some empty, are given in the order of their first
/// rows whatever run holds them, a batch's group as the values its
/// columns hold of it, a null and a text with its trailing space among
/// them; and a run of a batch gives the same rows when asked for rows.
#[test]
fn runs_of_rows_and_of_batches_are_given_in_one_order() {
    let empty = Run::Columns(Batch { columns: vec![Column::Ints(Ints::new(Vec::new(), 0, None))], rows: 0 }, Vec::new());
    assert!(empty.is_empty() && Run::Rows(Vec::new()).is_empty() && !columns().is_empty() && !rows().is_empty());
    let given: Vec<String> = Ordered::of(vec![rows(), empty, columns(), Run::Rows(Vec::new())], Vec::new())
        .unwrap()
        .map(|row| format!("{:?}", row.unwrap()))
        .collect();
    let text = |s: &str| Value::Char(s.into());
    let expected = [
        vec![exact(10), text("a")],
        vec![exact(1), text("x")],
        vec![Value::Null, text("bc")],
        vec![exact(5), text("y")],
        vec![exact(30), text("dd ")],
    ];
    assert_eq!(given, expected.iter().map(|row| format!("{row:?}")).collect::<Vec<_>>());
    let as_rows: Vec<(u64, String)> = columns().into_rows().into_iter().map(|(first, row)| (first, format!("{row:?}"))).collect();
    let firsts: Vec<u64> = as_rows.iter().map(|g| g.0).collect();
    assert_eq!(firsts, [0, 3, 9]);
    assert_eq!(as_rows.last().map(|g| g.1.clone()), expected.last().map(|row| format!("{row:?}")));
}
