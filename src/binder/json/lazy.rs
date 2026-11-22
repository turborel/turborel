//! [ISO/IEC 9075-2:2023 §9.48 GR4c iii, GR5d] JSON_VALUE of a DEFAULT
//! that can raise an exception condition, bound so that the DEFAULT's
//! `<value expression>` VE is evaluated only when its behavior is taken.
//!
//! A DEFAULT that draws a value of a sequence generator is left an
//! argument, with the operator's other DEFAULTs: NEXT VALUE FOR draws once
//! of each row whatever is taken [§6.14 GR1a], and is in no CASE [SR2].
//!
//! A call evaluates every argument first, so such a VE is no argument.
//! The operator is bound as a CASE of which behavior it takes
//! (`JsonValueTaken`, `functions/json/lazy.rs`):
//!
//! ```text
//! CASE WHEN taken = 0 THEN JSON_VALUE (... ERROR ON EMPTY ERROR ON ERROR)
//!      WHEN taken = 1 THEN <ON EMPTY branch>
//!      ELSE CAST (VE2 AS DT) END
//! ```
//!
//! where the first WHEN's call is the operator with its DEFAULTs ERROR,
//! never taken there. \[GR4c iii\] The ON EMPTY branch is `CAST (VE1 AS
//! DT)`, of which an exception condition is replaced as ON ERROR says: if
//! ON ERROR is not ERROR, the branch is `CASE WHEN <the cast fails> THEN
//! <ON ERROR's value> ELSE CAST (VE1 AS DT) END` (`JsonCastFails`).
//! \[GR5d i\] An exception of `CAST (VE2 AS DT)` is raised. \[DESIGN\] An
//! exception VE raises as it is evaluated, before the cast, is raised as
//! it is: a CASE catches nothing, and §9.48 replaces what "this `<cast
//! specification>`" raises, read as the cast's own.
//!
//! The operator's path is evaluated twice of a row that takes no DEFAULT,
//! so a DEFAULT that can neither raise nor draw is still an argument
//! (`binder/json.rs`), as no row could tell when it is evaluated.

use std::sync::Arc;

use super::super::Binder;
use crate::error::Result;
use crate::functions::{JsonCall, JsonCastFails, JsonOperation, JsonValueTaken, ScalarRef, TAKEN_EMPTY, TAKEN_VALUE, ValueSlot};
use crate::json::Path;
use crate::logical::{Cond, Expr};
use crate::value::{CompOp, DataType, Decimal, Value};

/// A JSON_VALUE as `binder/json.rs` bound it: its path's text and parse,
/// its PASSING arguments' identifiers and FORMAT JSON, and its operation.
pub(super) struct Bound<'a> {
    /// The `<JSON path specification>` as written.
    pub(super) text: &'a str,
    /// It parsed.
    pub(super) path: &'a Path,
    /// Each `<JSON argument>`'s identifier, and whether it is FORMAT JSON.
    pub(super) passing: &'a [(String, bool)],
    /// The operation, `JsonOperation::Value` if JSON_VALUE.
    pub(super) operation: &'a JsonOperation,
}

impl Binder<'_> {
    /// The JSON_VALUE `bound`, of its operands `exprs` of `types` bound,
    /// as the module's CASE, if a DEFAULT of it can raise an exception
    /// condition and none draws a value; none if none can raise, one
    /// draws, or it is not JSON_VALUE.
    ///
    /// # Errors
    ///
    /// As a DEFAULT's cast to the returned type is bound.
    pub(super) fn lazy_value(&self, bound: &Bound<'_>, exprs: &[Expr], types: &[DataType]) -> Result<Option<Expr>> {
        let JsonOperation::Value { returning, on_empty, on_error } = bound.operation else { return Ok(None) };
        let arguments = bound.passing.len().saturating_add(1);
        let (Some((operands, defaults)), Some(default_types)) = (exprs.split_at_checked(arguments), types.get(arguments..)) else {
            return Ok(None);
        };
        if defaults.iter().any(Expr::draws) || !defaults.iter().any(Expr::can_raise) {
            return Ok(None);
        }
        let mut written = Vec::with_capacity(defaults.len());
        for (e, t) in defaults.iter().zip(default_types) {
            let (cast, _) = self.cast_bound((e.clone(), t.clone()), returning.clone())?;
            written.push((e.clone(), cast));
        }
        let mut written = written.into_iter();
        let empty = (*on_empty == ValueSlot::Default).then(|| written.next()).flatten();
        let error = (*on_error == ValueSlot::Default).then(|| written.next()).flatten().map(|(_, cast)| cast);
        let call = |operation| JsonCall::new(bound.text, bound.path.clone(), bound.passing.to_vec(), operation);
        let taken = Expr::Call(ScalarRef(Arc::new(JsonValueTaken::new(call(bound.operation.clone())))), operands.to_vec());
        let none_taken = JsonOperation::Value { returning: returning.clone(), on_empty: unless(*on_empty), on_error: unless(*on_error) };
        let value = Expr::Call(ScalarRef(Arc::new(call(none_taken))), operands.to_vec());
        let empty = empty.map(|(raw, cast)| on_empty_branch(raw, cast, (returning, *on_error), error.clone()));
        let otherwise = match (empty, error) {
            (Some(empty), Some(error)) => Expr::Case(vec![(is(&taken, TAKEN_EMPTY), empty)].into(), Box::new(error)),
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => return Ok(None),
        };
        Ok(Some(Expr::Case(vec![(is(&taken, TAKEN_VALUE), value)].into(), Box::new(otherwise))))
    }
}

/// The slot `slot`, ERROR if DEFAULT: the call that gives the value is
/// evaluated only if no DEFAULT is taken.
fn unless(slot: ValueSlot) -> ValueSlot {
    match slot {
        ValueSlot::Default => ValueSlot::Error,
        ValueSlot::Error | ValueSlot::Null => slot,
    }
}

/// Whether `taken`, a `JsonValueTaken`'s call, is `code`.
fn is(taken: &Expr, code: i128) -> Cond {
    Cond::Compare(taken.clone(), CompOp::Eq, Expr::Const(Value::Exact(Decimal::new(code, 0))))
}

/// [§9.48 GR4c iii, GR5] The ON EMPTY branch of VE1 `raw`, cast to the
/// returned type `to` as `cast`: an exception condition of the cast
/// replaced as `on_error` says, by the null value or ON ERROR's `error`.
fn on_empty_branch(raw: Expr, cast: Expr, (to, on_error): (&DataType, ValueSlot), error: Option<Expr>) -> Expr {
    let replaced = match on_error {
        ValueSlot::Error => return cast,
        ValueSlot::Null => Expr::Const(Value::Null),
        ValueSlot::Default => error.unwrap_or(Expr::Const(Value::Null)),
    };
    let fails = Expr::Call(ScalarRef(Arc::new(JsonCastFails::new(to.clone()))), vec![raw]);
    Expr::Case(vec![(Cond::Value(fails), replaced)].into(), Box::new(cast))
}

#[cfg(test)]
mod tests;
