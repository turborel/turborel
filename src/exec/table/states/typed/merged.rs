//! \[DESIGN\] (RULES.md D1110) Columns of states merged a column at a
//! time: the groups of one table's column added to another's in one loop,
//! the kind of both and their scale compared once, not for each group. A
//! grouping run in parallel merges each morsel's groups into its
//! partition's so (`table/made.rs`).

use super::{Approx, Exact, Typed};
use crate::error::Result;
use crate::functions::NO_FLOATS;
use crate::storage::mask::index;

impl Typed {
    /// Adds `more` groups', fed nothing.
    pub(in crate::exec::table) fn grow_by(&mut self, more: usize) {
        match self {
            Typed::Count(counts) => counts.resize(counts.len().saturating_add(more), 0),
            Typed::Exact(exact) => {
                exact.totals.grow_by(more);
                exact.counts.resize(exact.counts.len().saturating_add(more), 0);
            }
            Typed::Approx(approx) => {
                approx.sums.resize(approx.sums.len().saturating_add(more), NO_FLOATS);
                approx.counts.resize(approx.counts.len().saturating_add(more), 0);
            }
            Typed::Best(best) => best.grow_by(more),
        }
    }

    /// Room for `more` groups more.
    pub(in crate::exec::table) fn reserve(&mut self, more: usize) {
        match self {
            Typed::Count(counts) => counts.reserve(more),
            Typed::Exact(exact) => {
                exact.totals.reserve(more);
                exact.counts.reserve(more);
            }
            Typed::Approx(approx) => {
                approx.sums.reserve(more);
                approx.counts.reserve(more);
            }
            Typed::Best(best) => best.reserve(more),
        }
    }

    /// The states of the groups at `order`, in that order.
    pub(in crate::exec::table) fn gathered(&self, order: &[u32]) -> Typed {
        let counts = |held: &[i64]| order.iter().map(|&i| held.get(index(i)).copied().unwrap_or(0)).collect();
        match self {
            Typed::Count(held) => Typed::Count(counts(held)),
            Typed::Exact(exact) => Typed::Exact(Exact {
                kind: exact.kind,
                totals: exact.totals.gathered(order),
                counts: counts(&exact.counts),
                scale: exact.scale,
                result: exact.result.clone(),
            }),
            Typed::Approx(approx) => Typed::Approx(Approx {
                mean: approx.mean,
                sums: order.iter().map(|&i| approx.sums.get(index(i)).copied().unwrap_or(NO_FLOATS)).collect(),
                counts: counts(&approx.counts),
                result: approx.result.clone(),
            }),
            Typed::Best(best) => Typed::Best(best.gathered(order)),
        }
    }

    /// \[DESIGN\] (RULES.md D1337) COUNT's counts, a group a slot, if it
    /// is a column of counts: what a reader of states where they lie
    /// takes as the integers they are, where a merge into states fed
    /// nothing would copy each.
    pub(in crate::exec::table::states) fn counts(&self) -> Option<&[i64]> {
        if let Typed::Count(counts) = self { Some(counts) } else { None }
    }

    /// Merges each group at `places` of `other` into the group of `groups`
    /// beside it, if both are of one kind and scale: whether they were,
    /// nothing merged if not.
    ///
    /// # Errors
    ///
    /// A sum past 128 bits.
    pub(in crate::exec::table) fn merge_all(&mut self, groups: &[u32], other: &Typed, places: &[u32]) -> Result<bool> {
        match (self, other) {
            (Typed::Count(counts), Typed::Count(theirs)) => {
                for (&g, &i) in groups.iter().zip(places) {
                    if let (Some(count), Some(&n)) = (counts.get_mut(index(g)), theirs.get(index(i))) {
                        *count = count.saturating_add(n);
                    }
                }
                Ok(true)
            }
            (Typed::Exact(exact), Typed::Exact(theirs)) => exact.merge_all(groups, theirs, places),
            (Typed::Best(best), Typed::Best(theirs)) => Ok(best.merge_all(groups, theirs, places)),
            (Typed::Count(_) | Typed::Exact(_) | Typed::Approx(_) | Typed::Best(_), _) => Ok(false),
        }
    }
}

impl Exact {
    /// See `Typed::merge_all`: of the other's kind and scale, a group
    /// that took no value merged as nothing. Each total is taken as
    /// `merge_from` takes it, whichever form either column holds its
    /// totals in, so a group merged alone and one merged among others are
    /// left the same (RULES.md D1308).
    fn merge_all(&mut self, groups: &[u32], other: &Exact, places: &[u32]) -> Result<bool> {
        let Some(scale) = other.scale else { return Ok(false) };
        if other.kind != self.kind || self.scale.is_some_and(|held| held != scale) {
            return Ok(false);
        }
        for (&g, &i) in groups.iter().zip(places) {
            let (Some(&held), Some(units)) = (other.counts.get(index(i)), other.totals.get(index(i))) else { continue };
            if held != 0 {
                self.scale = Some(scale);
                self.take(index(g), units, held)?;
            }
        }
        Ok(true)
    }
}
