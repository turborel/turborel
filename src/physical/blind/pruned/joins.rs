//! A plan's joins pruned (`blind/pruned.rs`, RULES.md D1167): each
//! operand, built or probing, carrying the columns read above the join
//! and those its keys and its conditions read, and no other.

use super::super::super::{JoinKey, PhysicalPlanner, Plan, SemiJoin};
use super::super::width::Sized;
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
/// and its right's `right` past the left's `lw` columns; of an operand
/// whose are not known, each of its `rw` columns.
fn beside(left: Live, right: Live, (lw, rw): (usize, usize)) -> Vec<usize> {
    let mut out = left.unwrap_or_else(|| (0..lw).collect());
    out.extend(right.unwrap_or_else(|| (0..rw).collect()).into_iter().map(|c| c.saturating_add(lw)));
    out
}

impl PhysicalPlanner<'_> {
    /// Prunes `plan`, an inner hash join or a product, of the widths
    /// `sizes`: each operand to the columns read of it above and by the
    /// join's keys.
    pub(super) fn join_pruned(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        let (Some(lw), Some(rw)) = (sizes.of(0), sizes.of(1)) else { return self.read_whole(plan) };
        let keys: &[JoinKey] = if let Plan::HashJoin { keys, .. } = &*plan { keys } else { &[] };
        let (of_left, of_right) = split(needed, keys, (lw, &[]));
        let (Plan::HashJoin { left, right, .. } | Plan::Product { left, right, .. }) = plan else { return None };
        let (ll, lr) = (self.operand_pruned(left, sizes.kid(0), &of_left), self.operand_pruned(right, sizes.kid(1), &of_right));
        Some(beside(ll, lr, (lw, rw)))
    }

    /// Prunes `plan`, an outer join, of the widths `sizes`: each operand to
    /// the columns read of it above and by the join's keys and conditions,
    /// its own conditions among them (`Own`); whole if a subquery is in a
    /// condition.
    pub(super) fn outer_pruned(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        let Plan::OuterJoin { left, right, keys, cond, own, widths, .. } = plan else { return None };
        let (of_l, of_r) = own.as_deref().map_or((None, None), |(l, r)| (l.as_ref(), r.as_ref()));
        if [cond.as_ref(), of_l, of_r].into_iter().flatten().any(Cond::has_subquery) {
            return self.read_whole(plan);
        }
        let (mut of_left, mut of_right) = split(needed, keys, (widths.0, &[cond.as_ref()]));
        if let Some(c) = of_l {
            cond_columns(c, &mut of_left);
        }
        if let Some(c) = of_r {
            cond_columns(c, &mut of_right);
        }
        let (ll, lr) = (self.operand_pruned(left, sizes.kid(0), &of_left), self.operand_pruned(right, sizes.kid(1), &of_right));
        Some(beside(ll, lr, *widths))
    }

    /// Prunes `semi`, a semi join or an antijoin, of the widths `sizes`,
    /// which gives its left's columns: each operand to the columns read of
    /// it, or, if a subquery is in its condition, as a plan of its own.
    pub(super) fn semi_pruned(&self, semi: &mut SemiJoin, sizes: &Sized, needed: &[usize]) -> Live {
        if semi.cond.as_ref().is_some_and(Cond::has_subquery) {
            self.prune(&mut semi.left);
            self.prune(&mut semi.right);
            return None;
        }
        let (of_left, of_right) = split(needed, &semi.keys, (semi.widths.0, &[semi.cond.as_ref()]));
        self.operand_pruned(&mut semi.right, sizes.kid(1), &of_right);
        self.operand_pruned(&mut semi.left, sizes.kid(0), &of_left)
    }
}
