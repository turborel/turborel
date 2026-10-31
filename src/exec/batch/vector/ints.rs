//! \[DESIGN\] RULES.md D31: an expression of exact numbers held as
//! integers (`storage::Ints`) evaluated a vector at a time as integers: a
//! column of them, an exact constant, a negation, a sum, a difference or a
//! product of them, of the scales and within the digits decimals give
//! [X3.135-1986 §5.9 SR4b, SR4c, GR6a] (`Decimal::checked_add`,
//! `checked_mul`), a null of a null [GR1]. A quotient is not, nor one whose
//! result goes past `MAX_EXACT_PRECISION` digits: those are evaluated of
//! values, which raise of it. A quotient's scale is each value's own
//! (`Decimal::checked_div`), which integers of one scale cannot hold.
//! Split from vector.rs (RULES.md R-SIZE-2).

use crate::logical::Expr;
use crate::storage::{Batch, Column, Ints};
use crate::value::{ArithOp, MAX_EXACT_PRECISION, Value, pow10};

/// An expression's integers over the rows of a batch given: a column's,
/// read in place, of those rows or all; a constant; or ones made, one a
/// row given.
pub(in crate::exec::batch) enum Lane<'b> {
    Column(&'b Ints, Option<&'b [usize]>),
    Const(i64, u32),
    Made(Ints),
}

impl Lane<'_> {
    /// The scale of its integers.
    pub(in crate::exec::batch) fn scale(&self) -> u32 {
        match self {
            Lane::Column(ints, _) => ints.scale,
            Lane::Const(_, scale) => *scale,
            Lane::Made(ints) => ints.scale,
        }
    }

    /// The units of the `n`th row given, none if it is null.
    pub(in crate::exec::batch) fn unit(&self, n: usize) -> Option<i64> {
        let of = |ints: &Ints, r: usize| (!ints.is_null(r)).then(|| ints.units.get(r).copied()).flatten();
        match self {
            Lane::Column(ints, rows) => of(ints, rows.map_or(Some(n), |r| r.get(n).copied())?),
            Lane::Const(u, _) => Some(*u),
            Lane::Made(ints) => of(ints, n),
        }
    }
}

/// Whether `e` may be evaluated as integers, its columns' vectors of them:
/// of columns and exact constants, negated, summed, subtracted or
/// multiplied, no deeper than `DEPTH`.
pub(in crate::exec::batch) fn evaluable(e: &Expr) -> bool {
    shaped(e, super::DEPTH)
}

/// `evaluable`'s work, of an expression up to `depth` deep.
fn shaped(e: &Expr, depth: usize) -> bool {
    let Some(deeper) = depth.checked_sub(1) else { return false };
    match e {
        Expr::Column(_) | Expr::Const(Value::Exact(_)) => true,
        Expr::Neg(operand) => shaped(operand, deeper),
        Expr::Arith(a, op, b) => *op != ArithOp::Div && shaped(a, deeper) && shaped(b, deeper),
        Expr::Const(_)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::Subquery(..) => false,
    }
}

/// The integers of `e` over the rows `rows` of `batch`, or all, each column
/// `c` of `e` the vector at `inputs[c]`, if `e` is of columns of integers
/// and exact constants, and computes, of those rows, nothing past the
/// digits of a decimal; of a column alone, its integers given again.
pub(in crate::exec::batch) fn lane(e: &Expr, inputs: &[usize], batch: &Batch, rows: Option<&[usize]>) -> Option<Ints> {
    let count = rows.map_or(batch.rows, <[usize]>::len);
    match lane_of(e, inputs, batch, rows)? {
        Lane::Made(ints) => Some(ints),
        // A column whole is its vector, copied, no row looked at.
        Lane::Column(ints, None) => Some(Ints::new(ints.units.clone(), ints.scale, ints.nulls.clone())),
        made @ (Lane::Column(..) | Lane::Const(..)) => each(count, made.scale(), |n| Some(made.unit(n))),
    }
}

/// As `lane`, its integers as `e` gives them: a column's read in place, a
/// constant once, none made but of the operations of `e`.
pub(in crate::exec::batch) fn lane_of<'b>(e: &'b Expr, inputs: &[usize], batch: &'b Batch, rows: Option<&'b [usize]>) -> Option<Lane<'b>> {
    made(e, (inputs, batch, rows), super::DEPTH)
}

/// `lane`'s work, of an expression up to `depth` deep.
fn made<'b>(e: &'b Expr, (inputs, batch, rows): (&[usize], &'b Batch, Option<&'b [usize]>), depth: usize) -> Option<Lane<'b>> {
    let deeper = depth.checked_sub(1)?;
    match e {
        Expr::Column(c) => match batch.columns.get(*inputs.get(*c)?)? {
            Column::Ints(ints) => Some(Lane::Column(ints, rows)),
            Column::Values(_) | Column::Floats(_) | Column::Texts(_) => None,
        },
        Expr::Const(Value::Exact(d)) => Some(Lane::Const(i64::try_from(d.units).ok()?, d.scale)),
        // [X3.135-1986 §5.9 GR4]
        Expr::Neg(operand) => {
            let x = made(operand, (inputs, batch, rows), deeper)?;
            let count = rows.map_or(batch.rows, <[usize]>::len);
            Some(Lane::Made(each(count, x.scale(), |n| Some(x.unit(n).map(i64::wrapping_neg)))?))
        }
        Expr::Arith(left, op, right) => {
            let operands = (made(left, (inputs, batch, rows), deeper)?, made(right, (inputs, batch, rows), deeper)?);
            arithmetic(*op, (&operands.0, &operands.1), rows.map_or(batch.rows, <[usize]>::len)).map(Lane::Made)
        }
        Expr::Const(_)
        | Expr::Call(..)
        | Expr::Outer(..)
        | Expr::Dynamic(_)
        | Expr::Case(..)
        | Expr::Cast(..)
        | Expr::Domain(..)
        | Expr::Subquery(..) => None,
    }
}

/// `op` of `x` and `y`, `count` rows of each, at the scale decimals give
/// it, if each result is within the digits of a decimal.
fn arithmetic(op: ArithOp, (x, y): (&Lane<'_>, &Lane<'_>), count: usize) -> Option<Ints> {
    let bound = pow10(MAX_EXACT_PRECISION)?;
    let fits = |units: i128| units.checked_abs().filter(|held| *held < bound).and_then(|_| i64::try_from(units).ok());
    match op {
        // [X3.135-1986 §5.9 SR4b] at the greater scale.
        ArithOp::Add | ArithOp::Sub => {
            let scale = x.scale().max(y.scale());
            let (fx, fy) = (pow10(scale.checked_sub(x.scale())?)?, pow10(scale.checked_sub(y.scale())?)?);
            let sign = if op == ArithOp::Sub { -1_i128 } else { 1_i128 };
            each(count, scale, |n| match (x.unit(n), y.unit(n)) {
                (Some(first), Some(second)) => {
                    let sum = i128::from(first).checked_mul(fx)?.checked_add(i128::from(second).checked_mul(fy)?.checked_mul(sign)?)?;
                    Some(Some(fits(sum)?))
                }
                (None, _) | (_, None) => Some(None),
            })
        }
        // [X3.135-1986 §5.9 SR4c] at the sum of the scales.
        ArithOp::Mul => {
            let scale = x.scale().checked_add(y.scale()).filter(|s| *s <= MAX_EXACT_PRECISION)?;
            each(count, scale, |n| match (x.unit(n), y.unit(n)) {
                (Some(first), Some(second)) => Some(Some(fits(i128::from(first).checked_mul(i128::from(second))?)?)),
                (None, _) | (_, None) => Some(None),
            })
        }
        ArithOp::Div => None,
    }
}

/// The integers of scale `scale` that `f` gives of each of `count` rows,
/// each a unit or a null: none if it gives none of one. Whether each is
/// null is kept only from the first null on, those before it not.
fn each(count: usize, scale: u32, f: impl Fn(usize) -> Option<Option<i64>>) -> Option<Ints> {
    let mut units = Vec::with_capacity(count);
    let mut nulls: Option<Vec<bool>> = None;
    for n in 0..count {
        let unit = f(n)?;
        units.push(unit.unwrap_or(0));
        match (&mut nulls, unit) {
            (Some(held), _) => held.push(unit.is_none()),
            (None, None) => {
                let mut held = Vec::with_capacity(count);
                held.resize(n, false);
                held.push(true);
                nulls = Some(held);
            }
            (None, Some(_)) => {}
        }
    }
    Some(Ints::new(units, scale, nulls))
}

// Unit tests: integers evaluated as their values are (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
