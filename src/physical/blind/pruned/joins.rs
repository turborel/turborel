//! A plan's joins pruned (`blind/pruned.rs`, RULES.md D1167): each
//! operand, built or probing, carrying the columns read above the join
//! and those its keys and its conditions read, and no other.

use super::super::super::{JoinKey, PhysicalPlanner, Plan, SemiJoin};
use super::Live;
use crate::logical::{Cond, cond_columns};

/// The columns of the rows of a join of operands `lw` columns wide read by
/// those above, `needed`, its keys, and its conditions `conds`, over the
/// pair's columns: of the left's and of the right's, each in its own
/// numbering.
fn split(needed: &[usize], keys: &[JoinKey], (lw, conds): (usize, &[Option<&Cond>])) -> (Vec<usize>, Vec<usize>) {
    let mut all = needed.to_vec();
    for cond in conds.iter().flatten() {
        cond_columns(cond, &mut all);
    }
    let mut left: Vec<usize> = all.iter().copied().filter(|&c| c < lw).collect();
    left.extend(keys.iter().map(|k| k.left));
    let mut right: Vec<usize> = all.iter().filter_map(|&c| c.checked_sub(lw)).collect();
    right.extend(keys.iter().map(|k| k.right));
    (left, right)
}

/// The columns of a join's rows that may hold a value, its left's `left`
/// and its right's `right` past the left's `lw` columns; none if either
/// is not known.
fn beside(left: Live, right: Live, lw: usize) -> Live {
    let mut out = left?;
    out.extend(right?.into_iter().map(|c| c.saturating_add(lw)));
    Some(out)
}

impl PhysicalPlanner<'_> {
    /// An inner hash join of `left` and `right` on `keys`, its operands
    /// pruned.
    pub(super) fn hash_pruned(
        &self,
        (left, right): (Plan, Plan),
        (keys, detail, est): (Vec<JoinKey>, String, f64),
        needed: &[usize],
    ) -> (Plan, Live) {
        let Some(lw) = self.sized(&left).width else {
            return self.whole(Plan::HashJoin { left: Box::new(left), right: Box::new(right), keys, detail, est });
        };
        let (of_left, of_right) = split(needed, &keys, (lw, &[]));
        let ((left, ll), (right, lr)) = (self.operand_pruned(left, &of_left), self.operand_pruned(right, &of_right));
        (Plan::HashJoin { left: Box::new(left), right: Box::new(right), keys, detail, est }, beside(ll, lr, lw))
    }

    /// The product of `left` and `right`, its operands pruned.
    pub(super) fn product_pruned(&self, (left, right): (Plan, Plan), est: f64, needed: &[usize]) -> (Plan, Live) {
        let Some(lw) = self.sized(&left).width else {
            return self.whole(Plan::Product { left: Box::new(left), right: Box::new(right), est });
        };
        let (of_left, of_right) = split(needed, &[], (lw, &[]));
        let ((left, ll), (right, lr)) = (self.operand_pruned(left, &of_left), self.operand_pruned(right, &of_right));
        (Plan::Product { left: Box::new(left), right: Box::new(right), est }, beside(ll, lr, lw))
    }

    /// `plan`, an outer join, its operands pruned, each to the columns
    /// read of it also by the conditions over its own (`Own`); whole if a
    /// subquery is in a condition.
    pub(super) fn outer_pruned(&self, plan: Plan, needed: &[usize]) -> (Plan, Live) {
        let Plan::OuterJoin { kind, left, right, keys, cond, own, widths, detail, est } = plan else { return (plan, None) };
        let (of_l, of_r) = own.as_deref().map_or((None, None), |(l, r)| (l.as_ref(), r.as_ref()));
        if [cond.as_ref(), of_l, of_r].into_iter().flatten().any(Cond::has_subquery) {
            return self.whole(Plan::OuterJoin { kind, left, right, keys, cond, own, widths, detail, est });
        }
        let (mut of_left, mut of_right) = split(needed, &keys, (widths.0, &[cond.as_ref()]));
        of_l.into_iter().for_each(|c| cond_columns(c, &mut of_left));
        of_r.into_iter().for_each(|c| cond_columns(c, &mut of_right));
        let ((left, ll), (right, lr)) = (self.operand_pruned(*left, &of_left), self.operand_pruned(*right, &of_right));
        let live = beside(ll, lr, widths.0);
        (Plan::OuterJoin { kind, left: Box::new(left), right: Box::new(right), keys, cond, own, widths, detail, est }, live)
    }

    /// `semi`, a semi join or an antijoin, which gives its left's columns,
    /// its operands pruned; whole if a subquery is in its condition.
    pub(super) fn semi_pruned(&self, semi: SemiJoin, needed: &[usize]) -> (Plan, Live) {
        if semi.cond.as_ref().is_some_and(Cond::has_subquery) {
            return self.whole(Plan::SemiJoin(Box::new(semi)));
        }
        let (of_left, of_right) = split(needed, &semi.keys, (semi.widths.0, &[semi.cond.as_ref()]));
        let (left, live) = self.operand_pruned(semi.left, &of_left);
        let (right, _) = self.operand_pruned(semi.right, &of_right);
        (Plan::SemiJoin(Box::new(SemiJoin { left, right, ..semi })), live)
    }
}
