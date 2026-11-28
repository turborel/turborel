//! Unit tests of the first filter over a scan told its conjuncts
//! (`judge/vetted.rs`, RULES.md D1266).

use std::sync::Arc;

use super::super::{Judged, Judging, Said};
use crate::db::Database;
use crate::exec::{Db, Runner};
use crate::logical::{Cond, Expr};
use crate::storage::{Batch, Column, Ints, Vetted};
use crate::txn::NO_TRANSACTION;
use crate::value::{CompOp, Decimal, Value};

fn int(n: i128) -> Expr {
    Expr::Const(Value::Exact(Decimal::new(n, 0)))
}

fn and(a: Cond, b: Cond) -> Cond {
    Cond::And(Box::new(a), Box::new(b))
}

fn like(column: usize, pattern: &str) -> Cond {
    Cond::Like(Expr::Column(column), Expr::Const(Value::Char(pattern.into())), None)
}

/// A batch of two columns of integers.
fn batch(first: &[i64], second: &[i64]) -> Batch {
    let column = |units: &[i64]| Column::Ints(Ints::new(units.to_vec(), 0, None));
    Batch { columns: vec![column(first), column(second)], rows: first.len() }
}

/// Verifies: [DESIGN] RULES.md D1266 the first filter over a scan knows,
/// of each conjunct it judges over vectors, the bit of the told conjunct
/// that is it, kept with the conjunct through their ordering: a LIKE is
/// judged last and still knows its own place, past the two bounds of its
/// prefix, which no conjunct of the filter is. A filter made any other
/// way knows none and hears no scan, nor does one of a condition that
/// can raise an error, of which a scan is told nothing.
#[test]
fn each_conjunct_judged_knows_its_told_place() {
    let below = Cond::Compare(Expr::Column(0), CompOp::Lt, int(5));
    let cond = and(like(1, "abc%"), and(below.clone(), Cond::IsNull(Expr::Column(3), true)));
    let judging = Judging::over_scan(&cond, &[0, 1, 3]);
    assert!(matches!(judging.judged.as_slice(), [Judged::Compare(..), Judged::IsNull(..), Judged::Like(..)]), "the LIKE last");
    assert_eq!(judging.told, [Vetted::bit(3), Vetted::bit(4), Vetted::bit(2)], "each with its own place");
    assert!(Judging::of(&cond, &[0, 1, 3]).told.is_empty(), "a filter not over a scan told its conjuncts");
    let raising =
        and(below.clone(), Cond::Like(Expr::Column(1), Expr::Const(Value::Char("a!x".into())), Some(Expr::Const(Value::Char("!".into())))));
    assert!(Judging::over_scan(&raising, &[0, 1]).told.iter().all(|told| *told == 0), "of a condition that can raise");
    let said = Arc::new(Said::default());
    assert!(judging.hearing(Some(&said)).said.is_some(), "the first filter over the scan hears it");
    assert!(Judging::of(&cond, &[0, 1, 3]).hearing(Some(&said)).said.is_none(), "no other filter does");
    assert!(judging.hearing(None).said.is_none());
}

/// Verifies: [DESIGN] RULES.md D1266 (MC/DC) a filter passes over a
/// conjunct of a batch only if the scan says it judged that conjunct's
/// own told conjunct exactly of the batch. The conjunct and the scan's
/// word are varied apart: with nothing said both conjuncts are judged;
/// with the first said, it is passed over, so a row it is false of is
/// kept as the scan vouched, and the second is still judged; with both
/// said, every row given is kept; with the bit of another place said,
/// both are judged; and the rows judged are those given, the scan's
/// kept rows. A batch the scan says nothing of after one it vouched for
/// is judged whole.
#[test]
fn a_conjunct_is_passed_over_only_of_a_batch_the_scan_judged_it_of() {
    let db = Db::from(Database::new().snapshot().unwrap());
    let runner = Runner::new(&db, NO_TRANSACTION);
    let cond = and(Cond::Compare(Expr::Column(0), CompOp::Lt, int(5)), Cond::Compare(Expr::Column(1), CompOp::Eq, int(1)));
    let said = Arc::new(Said::default());
    let judging = Judging::over_scan(&cond, &[0, 1]).hearing(Some(&said));
    let given = batch(&[1, 9, 3, 9], &[1, 1, 2, 2]);
    let kept = |exact: u64, rows: Option<&[u32]>| {
        said.say(&Vetted { rows: None, exact, partly: None });
        judging.kept_of(&given, rows, &runner).unwrap().as_slice().to_vec()
    };
    assert_eq!(kept(0, None), [0], "nothing said: both judged");
    assert_eq!(kept(Vetted::bit(0), None), [0, 1], "the comparison of the first column is not judged again");
    assert_eq!(kept(Vetted::bit(1), None), [0, 2], "nor that of the second, said alone");
    assert_eq!(kept(Vetted::bit(0) | Vetted::bit(1), None), [0, 1, 2, 3], "both said: every row given");
    assert_eq!(kept(Vetted::bit(0) | Vetted::bit(1), Some(&[1, 3])), [1, 3], "of the rows the scan kept");
    assert_eq!(kept(Vetted::bit(0), Some(&[1, 2, 3])), [1], "the conjunct not said is judged of the rows given");
    assert_eq!(kept(Vetted::bit(2), None), [0], "another conjunct's bit passes over neither");
    assert_eq!(kept(0, None), [0], "a batch nothing is said of, after batches vouched for");
}

/// Verifies: [DESIGN] RULES.md D1266 a conjunct the scan judged of a
/// batch's first rows alone is judged by the filter of the rows from the
/// place the scan stopped at, and of none before it: a row before that
/// place is kept though the conjunct is false of it, as the scan
/// vouched, a row from it on is kept only if the conjunct is true of
/// it, and the other conjunct is judged of every row. With the place at
/// the batch's first row every row is judged, past its last none is, and
/// the same holds of the rows a filter before kept.
#[test]
fn a_conjunct_judged_of_the_first_rows_is_judged_of_the_rest() {
    let db = Db::from(Database::new().snapshot().unwrap());
    let runner = Runner::new(&db, NO_TRANSACTION);
    let cond = and(Cond::Compare(Expr::Column(0), CompOp::Lt, int(5)), Cond::Compare(Expr::Column(1), CompOp::Eq, int(1)));
    let said = Arc::new(Said::default());
    let judging = Judging::over_scan(&cond, &[0, 1]).hearing(Some(&said));
    let given = batch(&[1, 9, 3, 1, 1], &[2, 2, 1, 2, 1]);
    let kept = |from: u32, rows: Option<&[u32]>| {
        said.say(&Vetted { rows: None, exact: 0, partly: Some((Vetted::bit(1), from)) });
        judging.kept_of(&given, rows, &runner).unwrap().as_slice().to_vec()
    };
    assert_eq!(kept(2, None), [0, 2, 4], "row 0 as the scan vouched, rows 2 to 4 judged, row 1 left by the other conjunct");
    assert_eq!(kept(0, None), [2, 4], "stopped at the first row: every row judged");
    assert_eq!(kept(5, None), [0, 2, 3, 4], "past the last row: none judged");
    assert_eq!(kept(4, Some(&[0, 3, 4])), [0, 3, 4], "of the rows given: 0 and 3 vouched for, 4 judged");
    assert_eq!(kept(3, Some(&[0, 3, 4])), [0, 4], "row 3 judged, and left");
}
