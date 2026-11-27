//! \[DESIGN\] (RULES.md D1186) The states of a part of a set function
//! (D341) kept in a column. A grouping whose groups are the parts of
//! another, as the first of the two DISTINCT_AGGREGATES makes, computes
//! PARTIAL of each set function, which told the table nothing of what it
//! is, so each group held an accumulator of each on the heap, fed a
//! group's rows at a time. A part of COUNT, SUM, AVG, MAX or MIN is now
//! kept as that function's column of states is (`typed.rs`): fed, grown,
//! merged, written and sent to another thread as the function's own, as a
//! part's state is its function's. Only what a group gives differs, the
//! part: its state after the tag of one, or its result where it writes no
//! state, the bytes the part's accumulator gives (`functions::part_value`).

use super::{Boxed, Typed};
use crate::error::Result;
use crate::functions::{part_acc, part_value};
use crate::logical::AggCall;
use crate::value::Value;

/// The column of states of the function `agg` is a part of, of no group
/// yet, if `agg` is a part of a built-in function of a result a column
/// holds.
pub(super) fn column_of(agg: &AggCall) -> Option<Typed> {
    let (kind, result) = agg.func.0.part_of()?;
    Typed::of(Some(kind.0), &result)
}

/// Whether the states of `agg` kept in a column are a part's.
pub(super) fn is_part(agg: &AggCall) -> bool {
    agg.func.0.part_of().is_some()
}

/// The part group `g` of `typed` gives, as its accumulator finishes it.
///
/// # Errors
///
/// As the function's `finish`, of one that writes no state.
pub(super) fn finished(typed: &Typed, g: usize) -> Result<Value> {
    part_value(|out| typed.write(g, out), || typed.finish(g))
}

/// Each group's accumulator of the part `agg`, as it would be: the
/// function's, as the column holds it, given what a part is.
pub(super) fn boxed(typed: &Typed, agg: &AggCall) -> Boxed {
    typed.to_boxed_by(agg, part_acc)
}
