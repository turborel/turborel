//! \[DESIGN\] RULES.md D31: an expression of a batch's columns evaluated a
//! vector at a time: a column, a constant, a negation or an arithmetic
//! operation of them, each node once over the rows of the batch given, a
//! value at a time as a row's is [X3.135-1986 §5.9] (`value::arith`,
//! `Value::negate`), its operands' values read in place. Any other
//! expression, or one nested deeper than `DEPTH`, is not; nor is a batch's
//! of which it raises an error. The caller evaluates those a row at a time,
//! as row by row, so an error raised is the row's it would be.

pub(super) mod ints;

use crate::error::Result;
use crate::logical::Expr;
use crate::storage::Batch;
use crate::value::{Value, arith};

/// \[DESIGN\] The deepest expression evaluated a vector at a time: one
/// deeper, rare, is evaluated a row at a time, so no evaluation by vectors
/// adds a frame for each level of a deep one (RULES.md R-CF-6).
pub(super) const DEPTH: usize = 16;

/// The null a lane gives past its values, which none of its rows asks.
static NULL: Value = Value::Null;

/// An expression's values over the rows of a batch given: a column's, read
/// in place, of those rows or all; ones made, one a row given; or a
/// constant, every row's.
pub(super) enum Lane<'b> {
    Column(&'b [Value], Option<&'b [usize]>),
    Made(Vec<Value>),
    Const(&'b Value),
}

impl Lane<'_> {
    /// The value of the `n`th row given.
    pub(super) fn at(&self, n: usize) -> &Value {
        match self {
            Lane::Column(values, rows) => rows.map_or(Some(n), |r| r.get(n).copied()).and_then(|r| values.get(r)).unwrap_or(&NULL),
            Lane::Made(values) => values.get(n).unwrap_or(&NULL),
            Lane::Const(v) => v,
        }
    }
}

/// The values of `e` over the rows `rows` of `batch`, or all, each column
/// `c` of `e` the vector at `inputs[c]`: none if `e` is not evaluated by
/// vectors or raises an error of one of them.
pub(super) fn lane<'b>(e: &'b Expr, inputs: &[usize], batch: &'b Batch, rows: Option<&'b [usize]>) -> Option<Lane<'b>> {
    made(e, (inputs, batch, rows), DEPTH)?.ok()
}

/// `lane`'s work, of an expression up to `depth` deep: none if it is not
/// evaluated by vectors, else its values or the first error of them.
fn made<'b>(e: &'b Expr, (inputs, batch, rows): (&[usize], &'b Batch, Option<&'b [usize]>), depth: usize) -> Option<Result<Lane<'b>>> {
    let deeper = depth.checked_sub(1)?;
    let count = rows.map_or(batch.rows, <[usize]>::len);
    Some(match e {
        Expr::Column(c) => Ok(Lane::Column(batch.columns.get(*inputs.get(*c)?)?.values(), rows)),
        Expr::Const(v) => Ok(Lane::Const(v)),
        // [X3.135-1986 §5.9 GR4]
        Expr::Neg(operand) => {
            made(operand, (inputs, batch, rows), deeper)?.map(|x| Lane::Made((0..count).map(|n| x.at(n).clone().negate()).collect()))
        }
        // [X3.135-1986 §5.9 GR1, GR5]
        Expr::Arith(a, op, b) => {
            let (left, right) = (made(a, (inputs, batch, rows), deeper)?, made(b, (inputs, batch, rows), deeper)?);
            left.and_then(|x| right.map(|y| (x, y)))
                .and_then(|(x, y)| (0..count).map(|n| arith(*op, x.at(n), y.at(n))).collect::<Result<Vec<Value>>>().map(Lane::Made))
        }
        Expr::Call(..) | Expr::Outer(..) | Expr::Dynamic(_) | Expr::Case(..) | Expr::Cast(..) | Expr::Domain(..) | Expr::Subquery(..) => {
            return None;
        }
    })
}
