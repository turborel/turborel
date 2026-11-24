//! \[DESIGN\] Range joins (RULES.md D580): a join whose condition compares a
//! value of each operand by `<`, `<=`, `>` or `>=`, a band such as
//! `A.X BETWEEN B.Y - 5 AND B.Y + 5` or the overlap of two periods, planned
//! as one operator that sorts one operand by the compared value and has
//! each row of the other search the rows between its bounds, rather than
//! as a filter over a product, which judges every pair.
//!
//! The rows are those the filter keeps [X3.135-1986 §5.20 GR2b, §5.21
//! GR2], or of a LEFT outer join [ISO/IEC 9075:1992 §7.5] those and each
//! left row matching none with nulls, in the order of the left operand,
//! each row's matches in the right's, while the sorted rows fit the work
//! area (`exec/join/range.rs`). The operand sorted, the right, is the one
//! of fewer estimated rows, as a hash join's
//! built side is (`physical/join.rs`); the join of the operands swapped is
//! put back in order by a projection. Comparisons are judged once for each
//! row of an operand and the rest of the condition only on the pairs the
//! search finds, so the whole condition must be unable to raise by its
//! columns' declared types (RULES.md D15, D373): a part that can is made a
//! column of its operand over a guard first (`optimizer/hoisting.rs`), or
//! the join stays a nested loop.
//!
//! What the search answers, of the compared value of the sorted operand:
//! equalities of a column of each operand, which group its rows, as a hash
//! join's keys do (Reif and Neumann, "A Scalable and Generic Approach to
//! Range Joins", PVLDB 15(11), 2022, a sorted array per group being their
//! tree in one dimension); a lower and an upper bound of one value, each
//! open or closed; and one more comparison of another value, answered by
//! the greatest or least of it kept over the sorted rows (`Pruned`), which
//! makes the overlap `A.S < B.E AND B.S < A.E` a search of one bound and
//! that comparison. A `<period predicate>` of two `<period reference>`s
//! [ISO/IEC 9075-2:2023 §8.20] is the comparisons of their bounds it is
//! made of (`range/periods.rs`, RULES.md D581). Every other conjunct is the
//! rest of the condition, judged of each pair found.

mod cost;
mod outer;
mod periods;
mod planned;
mod recognized;
#[cfg(test)]
mod tests;

pub(super) use cost::range_cost;
pub(super) use periods::expanded_periods;
pub(super) use planned::{Made, range_plan};
pub(super) use recognized::{Ranged, ranged};

use super::{JoinKey, Plan};
use crate::logical::{Cond, Expr};
use crate::value::CompOp;

/// \[DESIGN\] A range join (RULES.md D580), as the module says.
#[derive(Debug, Clone)]
pub struct RangeJoin {
    /// Whether the left rows that match none are kept, of a LEFT join.
    pub outer: bool,
    /// The operand whose rows search, in its order.
    pub left: Plan,
    /// The operand sorted, read once.
    pub right: Plan,
    /// Equalities of a column of each operand, which group the sorted rows.
    pub keys: Vec<JoinKey>,
    /// The value the right's rows are sorted by, over the right's columns.
    pub sorted: Expr,
    /// The bound the sorted value is above, of each left row.
    pub lower: Option<Bound>,
    /// The bound the sorted value is below, of each left row.
    pub upper: Option<Bound>,
    /// One more comparison answered by the search.
    pub pruned: Option<Pruned>,
    /// The rest of the condition, over the pair's columns.
    pub cond: Option<Cond>,
    /// The whole condition, over the pair's columns: judged of every pair
    /// where the search cannot be made (`exec/join/range.rs`).
    pub full: Cond,
    /// The degrees of `left` and `right`, the widths of the nulls.
    pub widths: (usize, usize),
    /// The condition, as text.
    pub detail: String,
    /// Its estimated rows (`physical/cardinality.rs`).
    pub est: f64,
}

/// A bound of the sorted value: a value of the left row, which the sorted
/// value is past, or `closed`, past or equal to.
#[derive(Debug, Clone)]
pub struct Bound {
    /// The bound, over the left's columns.
    pub probe: Expr,
    /// Whether a sorted value equal to it is within.
    pub closed: bool,
}

/// A comparison `build op probe` of a value of the right row and one of
/// the left, `op` one of `<`, `<=`, `>` and `>=`.
#[derive(Debug, Clone)]
pub struct Pruned {
    /// The right's value, over its columns.
    pub build: Expr,
    /// How it compares.
    pub op: CompOp,
    /// The left's value, over its columns.
    pub probe: Expr,
}
