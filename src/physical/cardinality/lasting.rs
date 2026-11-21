//! \[DESIGN\] Estimates kept the whole planning (RULES.md D384, D385):
//! each pair of relations' correction and each column's most rows of one
//! value, of nodes of the plan being planned. A greedy order asks for
//! the rows of each prefix of a chain, and each asking would find them
//! again for the same relations; nodes not of the plan, which an asking
//! may make and drop, are never kept.

use std::collections::HashMap;

use super::Estimates;
use crate::hash::xxhash::Xxh3BuildHasher;
use crate::logical::LogicalPlan;

/// The address of `plan`, which a node keeps as long as the planning.
fn at(plan: &LogicalPlan) -> usize {
    std::ptr::from_ref(plan).addr()
}

/// A pair of relations on their columns: each one's address and columns.
type Pair = (usize, Vec<usize>, usize, Vec<usize>);

/// The estimates kept: each pair's correction, by its relations and
/// their columns, and each column's most rows of one value.
#[derive(Default)]
pub(super) struct Lasting {
    factors: HashMap<Pair, f64, Xxh3BuildHasher>,
    most: HashMap<(usize, usize), f64, Xxh3BuildHasher>,
}

impl Estimates<'_> {
    /// The correction of the pair of `r` and `s` on their columns, as
    /// `find` makes it, kept if both are of the plan being planned.
    pub(super) fn kept_factor(
        &self,
        (r, rc): (&LogicalPlan, &[usize]),
        (s, sc): (&LogicalPlan, &[usize]),
        find: impl FnOnce() -> f64,
    ) -> f64 {
        if !(self.planned.contains(&at(r)) && self.planned.contains(&at(s))) {
            return find();
        }
        let key = (at(r), rc.to_vec(), at(s), sc.to_vec());
        let known = self.lasting.borrow().factors.get(&key).copied();
        known.unwrap_or_else(|| {
            let factor = find();
            self.lasting.borrow_mut().factors.insert(key, factor);
            factor
        })
    }

    /// The most rows of one value of `plan`'s column `column`, as `find`
    /// makes it, kept if `plan` is of the plan being planned.
    pub(super) fn kept_most(&self, plan: &LogicalPlan, column: usize, find: impl FnOnce() -> f64) -> f64 {
        if !self.planned.contains(&at(plan)) {
            return find();
        }
        let known = self.lasting.borrow().most.get(&(at(plan), column)).copied();
        known.unwrap_or_else(|| {
            let most = find();
            self.lasting.borrow_mut().most.insert((at(plan), column), most);
            most
        })
    }
}
