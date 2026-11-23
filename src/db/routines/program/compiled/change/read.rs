//! \[DESIGN\] The columns a delta table's select list reads at one
//! execution (RULES.md D918). A CASE gives the result of its first WHEN
//! that is true and evaluates no other [ISO/IEC 9075:1992 §6.9 GR2]. Where
//! its conditions read nothing of the row, which of its results it gives
//! is known before the row is, so the row need hold the columns of that
//! result alone. A program that picks one column of ten by a variable
//! then has one decoded, not ten.
//!
//! A condition is judged ahead only if it is of constants and the frame's
//! values, cast, negated or added to. Evaluating it reads no table, draws
//! no value, runs no routine and raises no warning, so judging it twice
//! shows nowhere. One that fails is left to raise its error where the
//! select list is evaluated. The columns of any other CASE are all its
//! columns, as are those of every other expression.

use crate::exec::{Framed, NoSubqueries, eval_cond};
use crate::logical::{Cond, Expr, Truth, expr_columns};
use crate::txn::escrow::bit;
use crate::value::Value;

/// \[DESIGN\] How deep the select list is walked for its CASEs: one
/// nested deeper reads all its columns, as an expression of any other
/// shape does. Bound expressions nest a few levels.
const DEPTH: usize = 32;

/// The columns of the delta table's row the select list `outputs` reads,
/// the frame holding `frame`: a bit a column.
pub(super) fn read_now(outputs: &[Expr], frame: &[Value]) -> u64 {
    outputs.iter().fold(0, |mask, e| mask | read_of(e, frame, DEPTH))
}

/// Every column `e` reads, whatever its conditions give.
fn all_of(e: &Expr) -> u64 {
    let mut read = Vec::new();
    expr_columns(e, &mut read);
    read.into_iter().fold(0, |mask, c| mask | bit(c))
}

/// The columns `e` reads with the frame holding `frame`, its CASEs walked
/// `depth` deep.
fn read_of(e: &Expr, frame: &[Value], depth: usize) -> u64 {
    let Some(deeper) = depth.checked_sub(1) else { return all_of(e) };
    if let Expr::Column(c) = e {
        return bit(*c);
    }
    let Expr::Case(whens, otherwise) = e else {
        let mut mask = 0;
        e.for_each_child(&mut |child| mask |= read_of(child, frame, deeper));
        return mask;
    };
    for (cond, result) in whens.iter() {
        match judged(cond, frame) {
            Some(true) => return read_of(result, frame, deeper),
            Some(false) => {}
            None => return all_of(e),
        }
    }
    read_of(otherwise, frame, deeper)
}

/// Whether `cond` is true with the frame holding `frame`, if it is judged
/// ahead of its row: none of one that reads the row or more than values,
/// or that fails.
fn judged(cond: &Cond, frame: &[Value]) -> Option<bool> {
    // Of values alone, so of no column of the row either.
    let mut plain = cond.subqueries().is_empty();
    cond.for_each_expr(&mut |e| plain = plain && of_values(e, DEPTH));
    if !plain {
        return None;
    }
    let framed = Framed { inner: &NoSubqueries, frame, outer: &[] };
    eval_cond(cond, &Vec::new(), &framed).ok().map(|truth| truth == Truth::True)
}

/// Whether `e` is of constants and the frame's values alone, cast,
/// negated or in arithmetic, `depth` deep at most.
fn of_values(e: &Expr, depth: usize) -> bool {
    let Some(deeper) = depth.checked_sub(1) else { return false };
    let mut plain = matches!(e, Expr::Const(_) | Expr::Dynamic(_) | Expr::Cast(..) | Expr::Neg(_) | Expr::Arith(..));
    e.for_each_child(&mut |child| plain = plain && of_values(child, deeper));
    plain
}

// Unit tests: which columns a select list reads (RULES.md R-TEST-1).
#[cfg(test)]
mod tests;
