//! Unit tests of the columns a select list reads at one execution
//! (RULES.md R-TEST-1).

use super::read_now;
use crate::logical::{Cond, Expr};
use crate::txn::escrow::bit;
use crate::value::{CompOp, Decimal, Value};

fn int(n: i128) -> Value {
    Value::Exact(Decimal::new(n, 0))
}

/// The frame's slot `index`, an integer.
fn slot(index: usize) -> Expr {
    let data_type = crate::value::DataType::Exact { precision: 9, scale: 0, spelled: crate::value::Spelled::Numeric };
    Expr::Dynamic(crate::logical::Slot { index, data_type, at: 0 })
}

/// `CASE WHEN slot 0 = 1 THEN column 3 WHEN slot 0 = 2 THEN column 4 ELSE
/// column 5 END`.
fn picked() -> Expr {
    let when = |n: i128, column: usize| (Cond::Compare(slot(0), CompOp::Eq, Expr::Const(int(n))), Expr::Column(column));
    Expr::Case(vec![when(1, 3), when(2, 4)].into(), Box::new(Expr::Column(5)))
}

/// Verifies: [DESIGN] RULES.md D918; [ISO/IEC 9075:1992 §6.9 GR2]: a
/// CASE whose conditions compare the frame's values reads the columns of
/// the result it gives alone, of its first WHEN true, of a later one, of
/// its ELSE where none is true or one is unknown; an expression around
/// it reads its own columns beside; and several outputs read the columns
/// of each.
#[test]
fn a_case_over_the_frame_reads_the_columns_of_its_result() {
    assert_eq!(read_now(&[picked()], &[int(1)]), bit(3));
    assert_eq!(read_now(&[picked()], &[int(2)]), bit(4));
    assert_eq!(read_now(&[picked()], &[int(7)]), bit(5));
    assert_eq!(read_now(&[picked()], &[Value::Null]), bit(5), "unknown is not true");
    let around = Expr::Neg(Box::new(picked()));
    assert_eq!(read_now(&[around, Expr::Column(7)], &[int(2)]), bit(4) | bit(7));
    assert_eq!(read_now(&[], &[int(2)]), 0);
    assert_eq!(read_now(&[Expr::Const(int(1))], &[]), 0);
}

/// Verifies: [DESIGN] RULES.md D918: a CASE is read by all its columns,
/// its conditions' too, where one of them is not judged ahead of the row:
/// one that reads a column, one of more than constants and the frame's
/// values, here a truth value, and one that fails, its slot holding no
/// value. A WHEN before it that is true still decides alone.
#[test]
fn a_case_not_judged_ahead_reads_all_its_columns() {
    let all = bit(3) | bit(4) | bit(5);
    assert_eq!(read_now(&[picked()], &[]), all, "the slot has no value");
    let of_row = Cond::Compare(Expr::Column(0), CompOp::Eq, Expr::Const(int(1)));
    let reading = Expr::Case(vec![(of_row, Expr::Column(3))].into(), Box::new(Expr::Column(5)));
    assert_eq!(read_now(&[reading], &[int(1)]), bit(0) | bit(3) | bit(5));
    let truth = Cond::Compare(Expr::Truth(Box::new(Cond::Const(crate::logical::Truth::True))), CompOp::Eq, Expr::Const(Value::Bool(true)));
    let first = (Cond::Compare(slot(0), CompOp::Eq, Expr::Const(int(1))), Expr::Column(3));
    let valued = Expr::Case(vec![first, (truth, Expr::Column(4))].into(), Box::new(Expr::Column(5)));
    assert_eq!(read_now(std::slice::from_ref(&valued), &[int(2)]), all, "more than values");
    assert_eq!(read_now(&[valued], &[int(1)]), bit(3), "decided before it");
}
