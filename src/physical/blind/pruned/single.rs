//! A plan's operators of one input pruned (`blind/pruned.rs`, RULES.md
//! D1167): a scan, a filter, a projection, a sort, a limit and a grouping.

use super::super::super::{PhysicalPlanner, Plan};
use super::super::width::Sized;
use super::Live;
use crate::logical::{AggCall, Cond, Expr, expr_columns};

/// Prunes `plan`, a scan, of whose columns those above read `needed`, to
/// read those and the ones it is ordered by alone.
pub(super) fn scan_pruned(plan: &mut Plan, needed: &[usize]) -> Live {
    let Plan::SeqScan { columns, order, located, .. } = plan else { return None };
    // A row's id follows a located scan's columns (`LogicalPlan::Scan`),
    // which are left as they are.
    if *located {
        return None;
    }
    columns.retain(|c| needed.contains(c) || order.contains(c));
    Some(columns.clone())
}

impl PhysicalPlanner<'_> {
    /// Prunes `plan`, a filter, its input of the widths `sizes`: its input to `needed` and its condition's
    /// columns, or, if a subquery is in its condition, whole.
    pub(super) fn filter_pruned(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        let Plan::Filter { cond, input, .. } = plan else { return None };
        if cond.has_subquery() {
            return self.read_whole(plan);
        }
        self.pruned_in(input, sizes, &super::super::with_cond(needed, Some(cond)))
    }

    /// Prunes `plan`, a projection, its input of the widths `sizes`: its
    /// input to the columns it moves that
    /// are read above and those its other expressions read, or, if a
    /// subquery is in one, whole.
    pub(super) fn project_pruned(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        let Plan::Project { exprs, input, .. } = plan else { return None };
        if !exprs.iter().all(plain) {
            return self.read_whole(plan);
        }
        let live = self.pruned_in(input, sizes, &project_reads(exprs, needed))?;
        Some(projected_live(exprs, &live))
    }

    /// Prunes `plan`, a sort, its input of the widths `sizes`: its input to
    /// `needed` and its keys' columns, a key made of a column's own octets
    /// read of the column first (`octets.rs`, RULES.md D1185). A function
    /// of its own, so the walk's frame holds none of its locals.
    #[inline(never)]
    pub(super) fn sort_pruned(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        let Plan::Sort { keys, input, .. } = plan else { return None };
        super::octets::in_place(keys, input, needed);
        let reads: Vec<usize> = needed.iter().copied().chain(keys.iter().map(|k| k.column)).collect();
        self.operand_pruned(input, sizes, &reads)
    }

    /// Prunes `plan`, a limit, its input of the widths `sizes`: its input
    /// to `needed` and, of WITH TIES, the keys whose peers it keeps, which
    /// it reads of the rows it is given [ISO/IEC 9075-2:2023 §7.17 GR7d i].
    #[inline(never)]
    pub(super) fn limit_pruned(&self, plan: &mut Plan, sizes: &Sized, needed: &[usize]) -> Live {
        let Plan::Limit { limit, input, .. } = plan else { return None };
        let Some(peers) = limit.fetch.as_ref().and_then(|f| f.ties.as_ref()) else { return self.pruned_in(input, sizes, needed) };
        let reads: Vec<usize> = needed.iter().copied().chain(peers.iter().map(|k| k.column)).collect();
        self.pruned_in(input, sizes, &reads)
    }

    /// Prunes `plan`, a grouping, its input of the widths `sizes`: its
    /// input to the columns its groups and
    /// its set functions read, or, if a subquery is in one, whole.
    pub(super) fn aggregate_pruned(&self, plan: &mut Plan, sizes: &Sized) -> Live {
        let Plan::Aggregate { groups, aggs, input, .. } = plan else { return None };
        if !aggs.iter().all(plain_agg) {
            return self.read_whole(plan);
        }
        self.pruned_in(input, sizes, &super::super::super::grouping::reads(groups, aggs));
        None
    }
}

/// Whether `e` reads nothing but the columns it names: no subquery in it.
fn plain(e: &Expr) -> bool {
    !Cond::Value(e.clone()).has_subquery()
}

/// Whether the set function `a` reads nothing but the columns it names.
fn plain_agg(a: &AggCall) -> bool {
    a.arg.as_ref().is_none_or(plain)
        && a.filter.as_ref().is_none_or(|f| !f.has_subquery())
        && a.distribution.as_ref().is_none_or(|d| plain(&d.fraction))
}

/// The columns a projection of `exprs` reads of its input, those above it
/// reading its columns `needed`: a column moved to one of `needed`, and
/// those any other expression reads, whether read above or not, so no
/// error it raises is lost.
fn project_reads(exprs: &[Expr], needed: &[usize]) -> Vec<usize> {
    let mut reads = Vec::new();
    for (n, e) in exprs.iter().enumerate() {
        if let Expr::Column(c) = e {
            if needed.contains(&n) {
                reads.push(*c);
            }
        } else {
            expr_columns(e, &mut reads);
        }
    }
    reads
}

/// The columns of a projection of `exprs` that may hold a value, those of
/// its input `live`: a column moved of one of them, and any other
/// expression but the null value.
fn projected_live(exprs: &[Expr], live: &[usize]) -> Vec<usize> {
    let held = |e: &Expr| {
        if let Expr::Column(c) = e {
            live.contains(c)
        } else if let Expr::Const(v) = e {
            !v.is_null()
        } else {
            true
        }
    };
    exprs.iter().enumerate().filter(|(_, e)| held(e)).map(|(n, _)| n).collect()
}
