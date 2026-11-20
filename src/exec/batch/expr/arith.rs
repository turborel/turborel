//! \[DESIGN\] Arithmetic nodes over vectors (RULES.md D362): of two exact
//! operands as integers (`ints.rs`), but a quotient; of an approximate
//! one and a number in `f64` (`floats.rs`); of two constants once; of
//! anything else, each row's values by the rule a row's evaluation
//! applies [X3.135-1986 §5.9] (`value::arith`, `Value::negate`): of
//! intervals, datetimes, decimal floating-point numbers and quotients.
//! Both operands are evaluated whatever either gives, as a row's are, and
//! a null constant makes every row null [GR1].

use super::floats::{approx, is_approximate};
use super::ints::exact;
use super::vector::{Vector, values};
use crate::storage::Column;
use crate::value::{ArithOp, Value, arith as value_arith};

/// `op` of `x` and `y`, `count` rows of each: none if a row's would raise.
pub(super) fn arith(op: ArithOp, (x, y): (&Vector<'_>, &Vector<'_>), count: usize) -> Option<Vector<'static>> {
    if x.is_null_const() || y.is_null_const() {
        return Some(Vector::Const(Value::Null));
    }
    if let (Vector::Const(a), Vector::Const(b)) = (x, y) {
        return value_arith(op, a, b).ok().map(Vector::Const);
    }
    if op != ArithOp::Div
        && let (Some(a), Some(b)) = (exact(x), exact(y))
    {
        return super::ints::arith(op, (&a, &b), count).map(|ints| Vector::Owned(Column::Ints(ints)));
    }
    if (is_approximate(x) || is_approximate(y))
        && let (Some(a), Some(b)) = (approx(x), approx(y))
    {
        return super::floats::arith(op, (&a, &b), count).map(|floats| Vector::Owned(Column::Floats(floats)));
    }
    let (a, b) = (x.seq(), y.seq());
    let made = (0..count).map(|n| value_arith(op, a.get(n), b.get(n)).ok()).collect::<Option<Vec<Value>>>()?;
    Some(values(made))
}

/// [X3.135-1986 §5.9 GR4] `x` negated, `count` rows of it: none if a
/// row's negation does not fit an integer.
pub(super) fn negated(x: &Vector<'_>, count: usize) -> Option<Vector<'static>> {
    match x.column() {
        Some(Column::Ints(ints)) => return super::ints::negated(ints).map(|ints| Vector::Owned(Column::Ints(ints))),
        Some(Column::Floats(floats)) => return Some(Vector::Owned(Column::Floats(super::floats::negated(floats)))),
        Some(Column::Values(_) | Column::Texts(_) | Column::Datetimes(_) | Column::Bools(_) | Column::Nulls(_) | Column::Late(_))
        | None => {}
    }
    if let Vector::Const(v) = x {
        return Some(Vector::Const(v.negate()));
    }
    let seq = x.seq();
    Some(values((0..count).map(|n| seq.get(n).negate()).collect()))
}
