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

/// A run of `count` groups as a batch, the `n`th of first row
/// `from + 2n`, its one column that number.
fn stepped(from: i64, count: i64) -> Run {
    let numbers: Vec<i64> = (0..count).map(|n| from.saturating_add(n.saturating_mul(2))).collect();
    let firsts = numbers.iter().map(|&n| u64::try_from(n).unwrap()).collect();
    Run::Columns(Batch { columns: vec![Column::Ints(Ints::new(numbers, 0, None))], rows: usize::try_from(count).unwrap() }, firsts)
}

/// The grant of a database made for a test.
fn limit() -> Limit {
    let db = crate::db::Database::new();
    Limit::of(&crate::exec::Db::from(db.snapshot().unwrap()))
}

/// Verifies: [DESIGN] (RULES.md D1220) runs of batches are given as
/// batches of their groups in the order of their first rows, of two runs
/// whose groups alternate, across the batches' ends, in vectors of the
/// columns' types; and runs with rows among them give the same groups in
/// the same order, gathered.
#[test]
fn runs_of_batches_are_given_as_batches_in_one_order() {
    let numbers = |batches: Batches| -> (Vec<String>, Vec<usize>, bool) {
        let made: Vec<Batch> = batches.map(|kept| kept.unwrap().0).collect();
        let typed = made.iter().all(|batch| matches!(batch.columns.first(), Some(Column::Ints(_))));
        (
            made.iter().flat_map(|batch| batch.values(0).iter().map(|v| format!("{v:?}"))).collect(),
            made.iter().map(|b| b.rows).collect(),
            typed,
        )
    };
    let expected: Vec<String> = (0_i64..6000).map(|n| format!("{:?}", exact(n))).collect();
    let (given, sizes, typed) = numbers(batches((vec![stepped(1, 3000), stepped(0, 3000)], Vec::new()), &[0], &limit()).unwrap());
    assert_eq!(given, expected);
    assert_eq!(sizes, [BATCH_ROWS, BATCH_ROWS, 6000_usize.saturating_sub(BATCH_ROWS.saturating_mul(2))]);
    assert!(typed, "integers stay a vector of integers");
    let rows = Run::Rows((0_i64..6000).step_by(2).map(|n| (u64::try_from(n).unwrap(), vec![exact(n)])).collect());
    let (mixed, _, _) = numbers(batches((vec![stepped(1, 3000), rows], Vec::new()), &[0], &limit()).unwrap());
    assert_eq!(mixed, expected);
    assert_eq!(numbers(batches((Vec::new(), Vec::new()), &[0], &limit()).unwrap()).0, Vec::<String>::new());
}
