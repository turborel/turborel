//! \[DESIGN\] DISTINCT set functions computed by two groupings (RULES.md
//! D340). [ISO/IEC 9075-2:2023 §10.9 GR7b i] A DISTINCT set function
//! takes each value of its argument once, its redundant duplicates
//! eliminated by the comparison rules, so it is the function without DISTINCT of the groups of
//! its rows by the grouping columns and its argument: the rows grouped by
//! both first, then those groups by the grouping columns alone, each
//! giving its value of the argument once. Both groupings are groupings
//! the executor makes of batches, of one key each row, where a DISTINCT
//! function kept a key for each value of each group of each function.
//!
//! MAX and MIN take the greatest or least value [GR7d iv], which removing
//! values alike does not change, as a tie keeps the first value either
//! way (`functions/extremes.rs`), so their DISTINCT is dropped. Each other DISTINCT function's values are a slot:
//! its argument and its filter, which functions of both alike share. Of
//! more than one slot, each row is made once for each, a marker telling
//! which (`levels.rs`, RULES.md D342). A function without DISTINCT is
//! computed in parts by the first grouping and merged by the second
//! (`functions/partial.rs`, D341).
//!
//! The rule leaves alone an aggregation of an inverse distribution
//! function, whose values the grouping sorts, and one whose set functions
//! draw a value of a sequence generator or invoke a routine, which it
//! would move between plans and evaluate of other rows.

mod copies;
mod levels;
#[cfg(test)]
mod tests;

use super::{Rule, RuleContext};
use crate::functions::{Split, split_of};
use crate::lexer::kw;
use crate::logical::{AggCall, Cond, Expr, LogicalPlan};
use crate::value::collation::CollationRef;

/// The rule's name, as EXPLAIN shows it and its setting names it.
pub const DISTINCT_AGGREGATES: &str = "distinct_aggregates";

/// DISTINCT set functions computed by two groupings.
pub struct DistinctAggregates;

impl Rule for DistinctAggregates {
    fn name(&self) -> &'static str {
        DISTINCT_AGGREGATES
    }
    fn apply(&self, plan: &LogicalPlan, _ctx: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Aggregate { groups, collations, aggs, seeds, input } = plan else { return None };
        if !aggs.iter().any(|a| a.distinct) {
            return None;
        }
        let grouping = levels::Grouping { groups, collations, seeds };
        if aggs.iter().any(extreme) {
            let aggs = aggs.iter().map(|a| AggCall { distinct: a.distinct && !extreme(a), ..a.clone() }).collect();
            return Some(levels::regrouped(&grouping, aggs, (**input).clone()));
        }
        let shape = Shape::of(aggs)?;
        Some(levels::two_levels(&grouping, &shape, (**input).clone()))
    }
}

/// Whether `a` is MAX or MIN with DISTINCT [ISO/IEC 9075-2:2023 §10.9
/// GR7d iv]: the same of a value's duplicates or not.
fn extreme(a: &AggCall) -> bool {
    let name = a.func.0.name();
    a.distinct && (name == kw::MAX || name == kw::MIN)
}

/// The values of a DISTINCT set function: its argument, and its filter
/// [ISO/IEC 9075-2:2023 §10.9 GR4a].
struct Slot {
    /// The value the first grouping groups by: the argument, or, of
    /// `keyed`, the string its sort key was made of.
    arg: Expr,
    filter: Option<Cond>,
    /// \[DESIGN\] (RULES.md D1219) Of an argument that is the sort key
    /// of a string under a collation whose key is the text itself
    /// (`CollationRef::is_octets`), that collation. The first grouping
    /// groups by the string under it, which a table hashes and compares
    /// in place (RULES.md D1183), where it grouped by a binary string
    /// made of each row. Two strings share a group then exactly when
    /// their keys are equal, so the groups are the same.
    keyed: Option<CollationRef>,
}

impl Slot {
    /// The slot of a function of the argument `arg` and `filter`.
    fn of(arg: &Expr, filter: Option<Cond>) -> Slot {
        if let Expr::SortKey(of, collation) = arg
            && collation.is_octets()
        {
            return Slot { arg: (**of).clone(), filter, keyed: Some(collation.clone()) };
        }
        Slot { arg: arg.clone(), filter, keyed: None }
    }
}

/// The set functions of an aggregation, as the rule computes them.
struct Shape {
    /// The slots, in the order their first functions come.
    slots: Vec<Slot>,
    /// Of each function, its slot, if it is DISTINCT.
    of: Vec<Option<usize>>,
    /// How the functions without DISTINCT are computed in parts: by
    /// their states, unless one writes none.
    split: Split,
    /// The functions.
    aggs: Vec<AggCall>,
}

impl Shape {
    /// The shape of `aggs`, if the rule computes them.
    fn of(aggs: &[AggCall]) -> Option<Shape> {
        let (mut slots, mut keys, mut of, mut split) = (Vec::new(), Vec::new(), Vec::new(), Split::State);
        for a in aggs {
            if a.distribution.is_some() || !a.exprs().into_iter().all(movable) {
                return None;
            }
            let Some(arg) = a.arg.as_ref().filter(|_| a.distinct) else {
                // Every part of a function of one group is its whole.
                if split_of(&a.func, &a.result)? == Split::Whole {
                    split = Split::Whole;
                }
                of.push(None);
                continue;
            };
            let key = format!("{arg:?} {:?}", a.filter);
            let at = keys.iter().position(|k| *k == key).unwrap_or_else(|| {
                keys.push(key);
                slots.push(Slot::of(arg, a.filter.clone()));
                slots.len().saturating_sub(1)
            });
            of.push(Some(at));
        }
        Some(Shape { slots, of, split, aggs: aggs.to_vec() })
    }

    /// Whether a function is without DISTINCT.
    fn has_plain(&self) -> bool {
        self.of.iter().any(Option::is_none)
    }

    /// How many times each row is made: once for each slot, and once for
    /// the functions computed whole, if any.
    fn copies(&self) -> usize {
        self.slots.len().saturating_add(usize::from(self.split == Split::Whole && self.has_plain()))
    }
}

/// Whether `e` may be computed in another plan than its own: no subquery,
/// no value drawn of a sequence generator, no routine invoked in it.
fn movable(e: &Expr) -> bool {
    let mut moves = !matches!(e, Expr::Subquery(..) | Expr::NextValue(_) | Expr::Invoked(_));
    e.for_each_child(&mut |c| moves = moves && movable(c));
    moves
}
