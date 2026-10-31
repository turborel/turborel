//! \[DESIGN\] RULES.md D31: a LIKE whose pattern, or escape character, is
//! not a constant, judged over a batch's vectors: of a column, its
//! pattern a column or a constant, its escape character too if given, each
//! row's read in place, as text where the batch holds text, and matched
//! as a row's LIKE matches [X3.135-1986 §5.14 GR4, GR5] (`eval::
//! like_escape`, `like_truth`), each pattern split once for the statement
//! (`Subqueries::like`). A row whose judgment raises an error has none
//! judged so: the batch is then judged a row at a time, in order, so the
//! error is the row's it would be. Split from judge.rs (RULES.md R-SIZE-2).

use crate::exec::Subqueries;
use crate::exec::eval::{Escape, like_escape};
use crate::storage::{Batch, Cell};
use crate::value::Value;

/// A LIKE's pattern or escape character over a batch: the column at a
/// place of it, or a constant.
#[derive(Debug, Clone)]
pub(in crate::exec::batch) enum Operand {
    At(usize),
    Const(Value),
}

/// A character string of a batch's row, read in place.
enum Text<'b> {
    Null,
    Of(&'b str),
    /// A value not a string, nor null.
    Other,
}

/// The rows of `over`, rows of `batch`, of which the column at `at` LIKE
/// `pattern` ESCAPE `escape` is true, matched by `subqueries`: none if it
/// raises an error of one of them.
pub(super) fn kept(
    (at, pattern, escape): (usize, &Operand, Option<&Operand>),
    batch: &Batch,
    over: &[usize],
    subqueries: &dyn Subqueries,
) -> Option<Vec<usize>> {
    let mut kept = Vec::with_capacity(over.len());
    for &r in over {
        let escape = escape.map(|e| value_of(e, batch, r));
        // [ISO/IEC 9075:1992 §8.5 GR1] unknown of a null escape.
        let escape = match like_escape(escape.as_ref()).ok()? {
            Escape::None => None,
            Escape::Of(c) => Some(c),
            Escape::Null => continue,
        };
        let truth = match (text_of(&Operand::At(at), batch, r), text_of(pattern, batch, r)) {
            (Text::Of(x), Text::Of(y)) => subqueries.like(x, y, escape).ok()?,
            (Text::Null, _) | (_, Text::Null) => false,
            // [X3.135-1986 §5.14 SR1, SR2] not strings: a row's judgment
            // raises (`like_truth`).
            (Text::Other, _) | (_, Text::Other) => return None,
        };
        if truth {
            kept.push(r);
        }
    }
    Some(kept)
}

/// The string `operand` gives of row `r` of `batch`.
fn text_of<'b>(operand: &'b Operand, batch: &'b Batch, r: usize) -> Text<'b> {
    let cell = match operand {
        Operand::At(at) => batch.cell(*at, r),
        Operand::Const(v) => Cell::Value(v),
    };
    match cell {
        Cell::Text(s) => Text::Of(s),
        Cell::Value(Value::Char(s)) => Text::Of(s),
        Cell::Null => Text::Null,
        Cell::Value(v) => {
            if v.is_null() {
                Text::Null
            } else {
                Text::Other
            }
        }
        Cell::Exact(..) | Cell::Approx(_) => Text::Other,
    }
}

/// The value `operand` gives of row `r` of `batch`.
fn value_of(operand: &Operand, batch: &Batch, r: usize) -> Value {
    match operand {
        Operand::At(at) => batch.cell(*at, r).to_value(),
        Operand::Const(v) => v.clone(),
    }
}
