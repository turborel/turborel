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
//!
//! \[DESIGN\] (RULES.md D1280) The second grouping of the two takes a
//! part's column as the column it is. It keeps the function's states in a
//! column of the same kind and adds the part's slots to its own, each to
//! the slot of the group its group of the first fell in, by the loop two
//! partitions' columns are merged by. No part is made a value, and no
//! accumulator reads one back. That holds of a part whose function
//! writes its state from a column of counts or of exact totals, which is
//! merged in any order. Any other part is given as its value, as before.

use super::{Boxed, States, Typed};
use crate::error::Result;
use crate::exec::broken;
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

impl States {
    /// \[DESIGN\] (RULES.md D1280) Whether they are a part's, kept in a
    /// column a second grouping merges as it is. Such a column is of
    /// counts or of exact totals, whose function writes its state, so
    /// each group's part is that state and no result.
    pub(in crate::exec::table) fn is_merged_whole(&self) -> bool {
        matches!(self, States::Part(Typed::Count(_) | Typed::Exact(_)))
    }

    /// The column of a part's states, taken as it is; none of any other.
    pub(in crate::exec::table) fn into_part(self) -> Option<Typed> {
        if let States::Part(typed) = self { Some(typed) } else { None }
    }

    /// The states of no group yet of the function `part` is a part's
    /// column of, kept as that function's column.
    pub(in crate::exec::table) fn merging(part: &Typed) -> States {
        States::Typed(part.gathered(&[]))
    }

    /// \[DESIGN\] (RULES.md D1280) Merges the states at `places` of
    /// `part`, a part's column, each into the group of `groups` beside
    /// it, as two partitions' columns of one function are merged.
    ///
    /// # Errors
    ///
    /// As `Accumulator::merge`, a sum past 128 bits; SQLCODE -910 if they
    /// are not a column of the part's function.
    pub(in crate::exec::table) fn merge_part(&mut self, part: &Typed, (groups, places): (&[u32], &[u32])) -> Result<()> {
        let States::Typed(mine) = self else { return Err(broken("a part's column merged into no column")) };
        if mine.merge_all(groups, part, places)? {
            return Ok(());
        }
        // A column no group of which was fed a value tells no scale, and
        // is merged a group at a time, each adding nothing.
        for (&g, &i) in groups.iter().zip(places) {
            if !mine.merge_from(crate::storage::mask::index(g), part, crate::storage::mask::index(i))? {
                return Err(broken("a part's column of another function than its own"));
            }
        }
        Ok(())
    }
}
