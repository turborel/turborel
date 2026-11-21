//! \[DESIGN\] The rows of a semi join and of an antijoin (RULES.md D540,
//! D370). A left row is kept if its key is among the right's, so of each
//! equality of a left column and a right one the left keeps one in as many
//! as its distinct values outnumber the right's, the right's counted of
//! its rows as they are, filtered, by Cardenas' formula: the containment
//! of the smaller set of values in the larger every join estimate here
//! assumes. Of several equalities the least share is taken, as each must
//! hold. A condition of no equality keeps the share of left rows the pairs
//! it keeps would reach, at most all. An antijoin keeps the rest, at least
//! one row of a left operand that has one.

use super::lineage::Over;
use super::{Card, Estimates, at_least_one};
use crate::logical::{Cond, Expr, LogicalPlan, split_and};
use crate::value::CompOp;

impl Estimates<'_> {
    /// The card of the semi join, or of `anti` the antijoin, of `left` and
    /// `right` on `cond`.
    pub(in crate::physical) fn semi_card(&self, (anti, cond): (bool, &Cond), (left, right): (&LogicalPlan, &LogicalPlan)) -> Card {
        self.asked(|| self.semi_at((anti, cond), (left, right), 1))
    }

    /// The card of the semi join, or of `anti` the antijoin, of `left` and
    /// `right` on `cond`.
    pub(super) fn semi_at(&self, (anti, cond): (bool, &Cond), (left, right): (&LogicalPlan, &LogicalPlan), depth: usize) -> Card {
        let (l, r) = (self.card_at(left, depth), self.card_at(right, depth));
        let share = self.semi_share(cond, (left, right), (l.rows, r.rows));
        let kept = l.rows * share;
        let rows = if anti { at_least_one(l.rows - kept, l.rows) } else { at_least_one(kept, l.rows * r.rows) };
        Card { rows, upper: l.upper }
    }

    /// The share of the left's `rows.0` rows a semi join on `cond` keeps,
    /// the right giving `rows.1`.
    fn semi_share(&self, cond: &Cond, (left, right): (&LogicalPlan, &LogicalPlan), rows: (f64, f64)) -> f64 {
        let width = left.width();
        let mut conjuncts = Vec::new();
        split_and(cond.clone(), &mut conjuncts);
        let keyed = conjuncts
            .iter()
            .filter_map(|c| keyed_pair(c, width))
            .map(|(a, b)| {
                let theirs = self.groups(right, &[b], rows.1);
                let ours = self.distinct(left, a).max(1.0_f64);
                (theirs / ours).min(1.0_f64)
            })
            .reduce(f64::min);
        keyed.unwrap_or_else(|| (rows.1 * self.selectivity(cond, Over::Pair(left, right))).min(1.0_f64)).max(0.0_f64)
    }
}

/// Of a conjunct "a = b" of a column of each operand, the left operand
/// `width` columns wide, the left's column and the right's, numbered in
/// it.
fn keyed_pair(c: &Cond, width: usize) -> Option<(usize, usize)> {
    let Cond::Compare(Expr::Column(a), CompOp::Eq, Expr::Column(b)) = c else { return None };
    match (b.checked_sub(width), a.checked_sub(width)) {
        (Some(right), None) => Some((*a, right)),
        (None, Some(right)) => Some((*b, right)),
        (Some(_), Some(_)) | (None, None) => None,
    }
}
